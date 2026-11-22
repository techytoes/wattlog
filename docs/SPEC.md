# Mac Power, Sleep Diagnostics & Keep-Awake Utility — Research-Backed Feature Cut & Architecture Spec

*Synthesized on 2026-11-22 from 4 parallel research passes: HN #44745897 & Reddit insomnia pain points, macOS utility incumbent audit (Sleep Aid, Amphetamine, KeepingYouAwake, Stats, iStat Menus, AlDente, Battery Toolkit, Activity Monitor, coconutBattery), Apple Silicon zero-root IOKit/libproc/`IOReport` API verification, and Tauri v2 / `govijr/fix-storage` UX craft.*

---

## 1. Product Positioning

**Problem:** MacBook power users juggle 3–5 separate tools (`Amphetamine`/`Caffeine`, `Stats`/`iStat Menus`, `Activity Monitor`, `Sleep Aid`, and Terminal `pmset -g log`)—yet still get blindsided by overnight 20%–80% battery drain, "Hot Bag MacBook" sleep-blocking apps, fragmented Chrome/Electron helper rows with no per-app totals, and menu-bar monitors that themselves burn CPU/battery.

**Product Wedge:** One notch-friendly menu-bar app (`8.4W · ☕`) with a keyboard-first, `govijr/fix-storage`-inspired dark monospace dashboard that unifies:
1. Plain-English **Sleep Post-Mortem & Since-Last-Unplug Drain Timeline** (where Activity Monitor is blind)
2. **Safe-by-Default Keep-Awake (Caffeine)** + **Live Sleep-Blocker Guard**
3. **Grouped App Power / CPU / Memory Monitor** with 1-click **Quit / Force Kill**
4. **Zero `sudo`, zero root daemon, `<20 MB` idle RAM, drag-to-Trash uninstall**, plus a **5-second `--demo` / `--plain` / `--json` CLI**.

---

## 2. Evidence-Backed Feature Cut

### A. v1 MUST-SHIP (Zero-Root, Unsandboxed Developer ID)

| # | Feature | Pain Point Solved (Verified) | Native Zero-Root Implementation |
|---|---|---|---|
| **1** | **Sleep Post-Mortem & "Since Last Unplug" Timeline** | Activity Monitor stops logging during sleep and clears on 0% crash; `pmset -g log` is cryptic even to engineers (HN #44745897). | Parse `/usr/bin/pmset -g log` (`Sleep/Wake UUID`, `DarkWake`, `Charge:%`, `Assertions`) + lightweight 60s SQLite WAL background sampler while awake. Accurate timer counts from **discharge start** (avoiding `exelban/stats` #2798 bug). |
| **2** | **Plain-English Wake-Reason Decoder (18+ Tokens) + `pmset` Footgun Audit** | Users cannot decode `SMC.OutboxNotEmpty`, `AOP.OutboxNotEmpty`, `wifibt`, `NUB.SPMI`, `XHC`, `RTC`, `dasd`, `sharingd`, or catch misconfigured settings (like disabled *Wake for maintenance* or forgotten `disablesleep 1`). | Local decoder table translating raw hardware/kernel wake tokens into plain English + actionable guidance. Read-only `pmset -g custom` audit flags risky settings and offers a **"Copy Fix Command"** button (no root daemon!). |
| **3** | **Live Sleep Blockers & "Backpack Oven" Alert** | HN #44745897 top complaint: apps silently hold `NoIdleSleepAssertion` / `PreventSystemSleep` with zero macOS warning. | Native `IOPMCopyAssertionsByProcess()` (`AssertType`, `AssertName`, `AssertPID`, `AssertionOnSince`) mapped to app icons via `NSRunningApplication`. Shows active blockers in popover + optional notification when an unexpected app blocks sleep while on battery. |
| **4** | **Safe-by-Default Keep-Awake (Caffeine)** | KeepingYouAwake lacks smart triggers; Amphetamine has `"too many settings"` and `sudoers.d` friction; forgetting keep-awake drains/overheats laptops. | Native `IOPMAssertionCreateWithName` (`PreventUserIdleDisplaySleep` / `PreventUserIdleSystemSleep`), automatically kernel-released if app crashes. Guardrails ON by default: auto-off below 20% battery, optional auto-off on AC unplug, timers (30m / 1h / 2h / indefinite), and "while app running" (`NSWorkspace`). |
| **5** | **Grouped App Power, CPU, Memory (`phys_footprint`) & Safe Quit/Kill** | `Stats` & `iStat Menus` are read-only; Activity Monitor shows ungrouped Chrome/Electron helper soup; raw RAM % causes false panic; `kernel_task` confuses users. | `proc_listallpids` + `proc_pidinfo(PROC_PIDTBSDINFO)` + `proc_pidpath` + `NSRunningApplication` to roll helper processes up into parent apps (`Google Chrome (18 helpers) · 6.4 GB · 142% CPU`, expandable `└ mostly in Chrome Helper (Renderer)`). Uses `proc_pid_rusage` (`ri_phys_footprint`, `ri_pkg_idle_wkups`, `/usr/share/pmenergy/` weights) + true **Memory Pressure (Green/Yellow/Red)**. Default graceful `terminate()` / `SIGTERM`, 2-keypress `SIGKILL` for runaway helpers, hardcoded system-process denylist, and `kernel_task` thermal-throttling callout. |
| **6** | **3-Tier Live Wattage & Charger Telemetry** | Users want real Watts (system draw, CPU/GPU/ANE split, USB-C PD charger verification) and battery health without coconutBattery's raw-vs-macOS health panic. | Tier 1: `AppleSmartBattery` (`PowerTelemetryData.SystemLoad`, signed `Int64(InstantAmperage) × Voltage`, `AdapterDetails`, `BatteryData`). Tier 2 (Desktops/Fallback): `AppleSMC` read-only `PSTR`. Tier 3 (Component W): `macmon = { version = "0.10", default-features = false }` (`IOReport` / `AppleCLPC`) with automatic graceful hide if future macOS tightens entitlements. |
| **7** | **Single Compact Menu-Bar Footprint + CLI Mode** | Users stack 3–5 icons that collide with MacBook notches; devs love 5-second CLI inspection (`fix-storage`). | Single menu-bar item (`6.2W` + status dot / ☕ icon) updated every 2–3s only when changed. Same binary parses `--demo`, `--plain`, `--json` via `clap` *before* booting AppKit/WebKit. |

### B. v2 DEFERRED (Post-Launch, Validated by User Demand)

- **Travel / Backpack Mode Automations:** Optional Bluetooth toggle on sleep (with explicit Find My trade-off notice) to cure persistent `wifibt` / `SMC.OutboxNotEmpty` DarkWake loops.
- **USB-C Hub / External Monitor Overnight Drain Auditor:** Correlate `EC.USBC` / `EC.ACAttach` wakes and reverse trickle draw from powered-off monitors/docks.
- **30-Day Longitudinal Drain History & Export:** CSV/JSON diagnostics bundle export for bug reports or comparing macOS updates.
- **Custom Runaway Alerts & Per-App Keep-Awake Rules:** User-defined Wattage/CPU spike alerts and menu-bar formatting customization (potential optional $12–$19 one-time Pro pack later if demand warrants).

### C. EXPLICITLY CUT (Proven Technical & UX Traps)

| Cut Feature | Why It Is Cut |
|---|---|
| **SMC Battery Charge Limiter (80% cap, AlDente-style)** | Requires root `SMAppService` daemon writing Apple Silicon SMC keys (`CH0B`/`CH0C`); breaks across macOS updates (e.g., Tahoe), conflicts with native macOS 80% cap, causes battery-gauge desyncs & hot-bag bugs (`Battery-Toolkit` archived March 2026). |
| **Closed-Lid Keep-Awake on Battery (`disablesleep 1`)** | Hardware lid sensor overrides `IOPMAssertion`; bypassing requires `sudo pmset` / `/private/etc/sudoers.d/` hacks (Amphetamine Power Protect) and causes MacBook-in-backpack overheating (`KeepingYouAwake` declined `#19`, `#83`, `#262`). |
| **Privileged Root Helpers / `powermetrics` / Writing `pmset`** | Triggers Gatekeeper/permission loops, conflicts with other tools, and breaks drag-to-Trash uninstall. Replaced by zero-root `IOReport`/`AppleSmartBattery` + read-only `pmset` audit with "Copy Fix Command". |
| **WidgetKit / Notification Center Extensions & 1s Always-On Sensor/BT Polling** | Primary cause of `exelban/stats` monitoring-tax bugs (`#2901`, `#3167`: 30%–100%+ CPU loops). Skip widgets; run high-frequency per-process polling *only* while dashboard window is open. |
| **Raw "RAM % Used" Alarm & "Free RAM" Cleaner Buttons** | Triggers false anxiety and destroys credibility with power users. Show macOS **Memory Pressure** + swap + grouped `phys_footprint`. |
| **Mac App Store Distribution** | App Sandbox blocks `IOReport` (Guideline 2.5.1), `pmset -g log`, `proc_pid_rusage`, and quitting sibling apps. |

---

## 3. UI & Design System (`govijr/fix-storage` Inspired)

- **Canvas & Palette:** Dark graphite (`#0B0D10` canvas, `#11141A` pane, `#181C24` hover, `#232834` hairline border, `#E6EAF0` primary text, `#7C8596` muted). Semantic accents: `#22C55E` idle/sleep-ok, `#38BDF8` active/charging/GPU, `#F97316` runaway/DarkWake, `#EF4444` critical/sleep-blocker, `#64748B` system.
- **Typography:** Single monospace stack (`'SF Mono', 'JetBrains Mono', ui-monospace, monospace`) with `font-variant-numeric: tabular-nums` so live Watts/CPU/RAM never jitter.
- **Hero Dot-Matrix Bar (`<canvas>` Retina-scaled):** 5 rows × 64–80 columns (`3px` dots, `2px` gap).
  - *In Live view:* Proportional power mass (CPU, GPU, ANE, Display/System, Headroom).
  - *In Sleep / Since Last Charge view:* Chronological time buckets colored by Active, DarkWake, Deep Sleep, Sleep-Blocked, Charging.
- **2-Pane Compact Window (`760 × 500`, `titleBarStyle: Overlay`, `tauri-nspanel` floating over fullscreen Spaces):**
  - **Top Header (76px):** Hero Wattage (`8.4W · discharging · 6h 12m`), Battery Health/Adapter chip, Dot-Matrix Bar, Keep-Awake quick pill (`[c] Awake: OFF`).
  - **Left Rail (38%):** Compact category/mode buckets with mini dot-meters (`Since Last Unplug`, `Sleep Blockers`, `Keep Awake`, `Apps · CPU/RAM`, `System Audit`).
  - **Right Inspector (62%):** Ranked culprit rows with `└ mostly in ...` proof lines and semantic badges, e.g.:
    ```text
    ▸ Google Chrome (18 helpers)              6.4 GB · 42% CPU   [runaway]
      └ mostly in Chrome Helper (Renderer) · 4.1 GB · 840 wakeups/s
      [→ expand]  [q quit]  [x force-kill helper]
    ```
  - **Bottom Keybind Footer (28px):** `↑↓ navigate · → expand · g jump to worst culprit · c toggle awake · q quit app · / filter · Esc hide` (all destructive actions require 2-keypress confirmation).

---

## 4. Technical Architecture (Tauri v2 + Rust Core + React/TS)

### A. `<20 MB` Idle RAM Lifecycle (Zero Monitoring Tax)
1. **Rust 24/7 Core (`ActivationPolicy::Accessory` — no Dock icon):**
   - Runs `TrayIconBuilder` (monochrome template icon + throttled `6.2W` title updated every 2–3s only when changed).
   - Holds native `IOPMAssertion` (Keep-Awake) + battery safety watcher (`IOPSNotificationCreateRunLoopSource`).
   - Writes lightweight coarse samples every 60s to local SQLite (`rusqlite`, WAL mode, 7-day rolling retention).
   - Intercepts `RunEvent::ExitRequested` (`api.prevent_exit()`) so closing the UI never kills the tray daemon.
2. **Hybrid Lazy-Destroy `WebviewWindow`:**
   - On blur/close (`Esc` or `Cmd+W`), hide the window and start a **60-second idle timer**.
   - If still hidden after 60s, call **`window.destroy()`** to terminate `com.apple.WebKit.WebContent` (dropping idle footprint from ~70–120 MB down to **~15–25 MB**).
   - Never emit live IPC events when no window exists; on open, hydrate 24h history from SQLite in one `#[tauri::command]` and start 1.5s live process/power polling only while visible.
   - Skip `tauri-plugin-window-state` (conflicts with destroy/recreate); persist window bounds in SQLite.

### B. Core Rust Crate Stack (`Cargo.toml`)
- `macmon = { version = "0.10", default-features = false }` — sudoless Apple Silicon CPU/GPU/ANE/DRAM Watts + SMC/IOHID temps
- `objc2 = "0.6"`, `objc2-core-foundation = "0.3"`, `objc2-io-kit = { version = "0.3", features = ["ps", "pwr_mgt"] }` — `AppleSmartBattery`, `IOPMCopyAssertionsByProcess`, `IOPMAssertionCreateWithName`
- `libproc = "0.14"`, `mach2 = "0.4"`, `sysinfo = "0.37"` — `proc_pid_rusage` (`ri_phys_footprint`, wakeups, CPU), process-tree grouping, safe termination
- `rusqlite = { version = "0.32", features = ["bundled"] }` — local 7-day WAL time-series & sleep session cache
- `tauri-nspanel = "2.1"`, `tauri-plugin-single-instance = "2"`, `tauri-plugin-autostart = "2"`, `tauri-plugin-updater = "2"`
- `clap = { version = "4", features = ["derive"] }` — `--demo`, `--plain`, `--json` before GUI boot

### C. Distribution & Launch (Zero-Marketing-Overhead)
- **Local Dev / Prototype:** Unsigned local builds (`cargo tauri dev`) work immediately at $0 cost.
- **Public Launch (`Show HN` / `r/macapps`):** Automate signed & notarized `aarch64` `.dmg` + updater `latest.json` via `tauri-apps/tauri-action@v0` on GitHub Releases (requires $99/yr Apple Developer ID to avoid Sequoia Gatekeeper friction and Homebrew Cask rejection).
- **Homebrew Cask (`homebrew-tap`):** Installs both `YourApp.app` and symlinks `YourApp.app/Contents/MacOS/yourapp` into `/opt/homebrew/bin/yourapp` so `brew install --cask ... && yourapp --demo --plain` works in 5 seconds.
