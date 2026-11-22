import { invoke } from "@tauri-apps/api/core";
import type {
  ActionResult,
  DashboardSnapshot,
  KeepAwakeConfig,
  KeepAwakeState,
} from "./types";

export function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

let browserKeepAwake: KeepAwakeState = {
  active: false,
  mode: "display",
  remainingSecs: null,
  startedAtMs: null,
  autoOffBelowBatteryPct: 20,
  autoOffOnUnplug: false,
  watchAppName: null,
  watchAppRunning: false,
  lastAutoReleaseReason: null,
  assertionId: null,
};

function makeBrowserFallbackSnapshot(): DashboardSnapshot {
  const timeline = Array.from({ length: 48 }, (_, i) => {
    const hour = (15 + Math.floor(i / 2)) % 24;
    const min = (i % 2) * 30;
    const label = `${String(hour).padStart(2, "0")}:${String(min).padStart(2, "0")}`;
    if (i < 6) {
      return {
        label,
        state: "charging" as const,
        watts: 42.0,
        chargePct: 92 + i,
        durationSecs: 1800,
        detail: "USB-C PD 67W charging",
      };
    }
    if (i < 14) {
      return {
        label,
        state: "active" as const,
        watts: 9.6,
        chargePct: 98 - (i - 6),
        durationSecs: 1800,
        detail: "Active coding session",
      };
    }
    if (i < 34) {
      const pct = Math.max(71, 88 - Math.floor(((i - 14) * 17) / 20));
      if (i % 4 === 0) {
        return {
          label,
          state: "darkwake" as const,
          watts: 3.4,
          chargePct: pct,
          durationSecs: 1800,
          detail: "3 DarkWakes · wifibt SMC.OutboxNotEmpty",
        };
      }
      return {
        label,
        state: "sleep" as const,
        watts: 0.3,
        chargePct: pct,
        durationSecs: 1800,
        detail: "Deep Idle sleep",
      };
    }
    if (i < 39) {
      return {
        label,
        state: "blocked" as const,
        watts: 11.2,
        chargePct: 77 - (i - 34),
        durationSecs: 1800,
        detail: "Sleep blocked · Chrome WebRTC",
      };
    }
    return {
      label,
      state: "active" as const,
      watts: 8.4,
      chargePct: 74,
      durationSecs: 1800,
      detail: "Active on battery",
    };
  });

  const wifibt = {
    token: "wifibt",
    label: "Wi-Fi / Bluetooth Radio Wake",
    category: "bluetooth",
    explanation:
      "Wireless radio (AirDrop, Find My, Handoff, or Bluetooth accessory) woke the Mac during sleep.",
    tip: "Disable Power Nap / TCPKeepAlive on battery or turn off Bluetooth mouse before packing.",
    isInsomniaSuspect: true,
  };
  const rtc = {
    token: "NUB.SPMI",
    label: "PMU / RTC Timer Interrupt",
    category: "schedule",
    explanation: "Power Management Unit scheduled an internal RTC maintenance timer wakeup.",
    tip: "Disable Power Nap on battery (`sudo pmset -b powernap 0`).",
    isInsomniaSuspect: true,
  };

  return {
    timestampMs: Date.now(),
    demoMode: true,
    hostName: "MacBook-Pro.local",
    chipName: "Apple M3 Pro",
    macosVersion: "macOS 26.5",
    power: {
      systemWatts: 8.4,
      batteryWatts: -8.4,
      sourceTier: "AppleSmartBattery + macmon (preview)",
      state: "discharging",
      battery: {
        chargePct: 74,
        healthPct: 91.2,
        rawHealthPct: 89.4,
        cycleCount: 184,
        designCycleCount: 1000,
        voltageMv: 11890,
        amperageMa: -706,
        temperatureC: 31.4,
        timeRemainingMins: 372,
        externalConnected: false,
        isCharging: false,
        fullyCharged: false,
        nominalCapacityMah: 5699,
        rawMaxCapacityMah: 5586,
        designCapacityMah: 6249,
        currentCapacityMah: 4133,
      },
      adapter: {
        connected: false,
        watts: null,
        voltageMv: null,
        currentMa: null,
        name: null,
        description: "Unplugged · USB-C PD ready",
        isWireless: false,
      },
      components: {
        cpuWatts: 3.1,
        gpuWatts: 1.4,
        aneWatts: 0.2,
        dramWatts: 0.7,
        sysWatts: 8.4,
        displayOtherWatts: 3.0,
        cpuTempC: 47.2,
        gpuTempC: 43.8,
        cpuActivePct: 24.5,
        gpuActivePct: 11.2,
        fanRpm: 0,
        available: true,
      },
    },
    sleep: {
      sinceUnplug: {
        startedAt: "Nov 21 18:10",
        elapsedSecs: 77200,
        startChargePct: 98,
        currentChargePct: 74,
        totalDropPct: 24,
        activeSecs: 28400,
        sleepSecs: 48354,
        darkWakeSecs: 446,
        darkWakeCount: 28,
        drainPctPerHour: 1.1,
        onBatteryNow: true,
      },
      sessions: [
        {
          id: "demo-sleep-1",
          startTime: "Nov 21 23:18",
          endTime: "Nov 22 07:42",
          durationSecs: 30240,
          startChargePct: 88,
          endChargePct: 71,
          drainPct: 17,
          powerSource: "BATT",
          sleepReason: "Clamshell Sleep",
          darkWakeCount: 26,
          darkWakeTotalSecs: 418,
          worstWakeReason: wifibt,
          wakeEvents: [
            {
              timestamp: "Nov 21 23:44",
              kind: "DarkWake",
              durationSecs: 21,
              chargePct: 87,
              powerSource: "BATT",
              rawReason: "smc.sysState.Wake(0x70070000) wifibt SMC.OutboxNotEmpty/",
              decoded: wifibt,
            },
            {
              timestamp: "Nov 22 01:12",
              kind: "DarkWake",
              durationSecs: 19,
              chargePct: 83,
              powerSource: "BATT",
              rawReason: "NUB.SPMI0.SW3.IRQ nub-spmi0.0x02 rtc/Maintenance",
              decoded: rtc,
            },
          ],
          severity: "critical",
        },
      ],
      topWakeReasons: [
        { decoded: wifibt, count: 19, totalSecs: 304 },
        { decoded: rtc, count: 7, totalSecs: 114 },
      ],
      timeline,
      rawLogAvailable: true,
    },
    blockers: [
      {
        pid: 48210,
        processName: "Google Chrome Helper (Renderer)",
        appName: "Google Chrome",
        bundlePath: "/Applications/Google Chrome.app",
        assertionType: "PreventUserIdleDisplaySleep",
        assertionName: "WebRTC active audio/video session (meet.google.com)",
        details: "Blocks display & idle sleep while unplugged",
        durationSecs: 4920,
        severity: "critical",
        isSystemProtected: false,
        isSelfWattlog: false,
        blocksDisplay: true,
        blocksSystem: true,
      },
      {
        pid: 51904,
        processName: "caffeinate",
        appName: "caffeinate",
        bundlePath: null,
        assertionType: "PreventUserIdleSystemSleep",
        assertionName: "caffeinate command-line tool",
        details: "Started from terminal session",
        durationSecs: 1845,
        severity: "warning",
        isSystemProtected: false,
        isSelfWattlog: false,
        blocksDisplay: false,
        blocksSystem: true,
      },
      {
        pid: 344,
        processName: "powerd",
        appName: "powerd",
        bundlePath: null,
        assertionType: "PreventUserIdleSystemSleep",
        assertionName: "Powerd - Prevent sleep while display is on",
        details: "Normal macOS assertion while screen is awake",
        durationSecs: 8112,
        severity: "system",
        isSystemProtected: true,
        isSelfWattlog: false,
        blocksDisplay: false,
        blocksSystem: true,
      },
    ],
    keepAwake: browserKeepAwake,
    processes: {
      groups: [
        {
          id: "app:Google Chrome",
          name: "Google Chrome",
          primaryPid: 48102,
          bundlePath: "/Applications/Google Chrome.app",
          helperCount: 18,
          cpuPct: 42.4,
          footprintBytes: 6871947673,
          energyImpact: 54.8,
          wakeupsPerSec: 842,
          verdict: "runaway",
          isSystemProtected: false,
          isBlockingSleep: true,
          mostlyIn: "Chrome Helper (Renderer) · 4.1 GB · 842 wakeups/s",
          children: [
            {
              pid: 48210,
              ppid: 48102,
              name: "Google Chrome Helper (Renderer)",
              path: null,
              cpuPct: 28.6,
              footprintBytes: 4402341478,
              energyImpact: 38.2,
              wakeupsPerSec: 610,
              isSystemProtected: false,
            },
            {
              pid: 48144,
              ppid: 48102,
              name: "Google Chrome Helper (GPU)",
              path: null,
              cpuPct: 8.9,
              footprintBytes: 1288490188,
              energyImpact: 11.4,
              wakeupsPerSec: 164,
              isSystemProtected: false,
            },
          ],
        },
        {
          id: "app:Cursor",
          name: "Cursor",
          primaryPid: 39011,
          bundlePath: "/Applications/Cursor.app",
          helperCount: 11,
          cpuPct: 18.7,
          footprintBytes: 3435973836,
          energyImpact: 22.1,
          wakeupsPerSec: 312,
          verdict: "elevated",
          isSystemProtected: false,
          isBlockingSleep: false,
          mostlyIn: "Cursor Helper (Plugin) · 1.9 GB · 214 wakeups/s",
          children: [
            {
              pid: 39118,
              ppid: 39011,
              name: "Cursor Helper (Plugin)",
              path: null,
              cpuPct: 12.1,
              footprintBytes: 2040109465,
              energyImpact: 14.3,
              wakeupsPerSec: 214,
              isSystemProtected: false,
            },
          ],
        },
      ],
      memoryPressure: {
        level: "green",
        totalBytes: 34359738368,
        usedBytes: 21904333209,
        wiredBytes: 4509715660,
        compressedBytes: 3650722201,
        swapUsedBytes: 536870912,
        swapTotalBytes: 2147483648,
      },
      totalCpuPct: 81.8,
      totalFootprintBytes: 12401298635,
      kernelThermalNote: null,
    },
    audit: [
      {
        id: "demo-powernap",
        profile: "Battery Power",
        key: "powernap",
        currentValue: "1",
        recommendedValue: "0",
        severity: "warning",
        title: "Power Nap enabled on battery",
        explanation: "Triggers periodic DarkWakes on battery for Mail, iCloud, and Time Machine.",
        fixCommand: "sudo pmset -b powernap 0",
      },
      {
        id: "demo-tcpkeepalive",
        profile: "Battery Power",
        key: "tcpkeepalive",
        currentValue: "1",
        recommendedValue: "0",
        severity: "warning",
        title: "TCPKeepAlive active on battery (28 DarkWakes seen)",
        explanation: "Common cause of wifibt + SMC.OutboxNotEmpty overnight wake loops.",
        fixCommand: "sudo pmset -b tcpkeepalive 0",
      },
    ],
    history: [],
  };
}

export async function fetchSnapshot(forceSleepRefresh = false): Promise<DashboardSnapshot> {
  if (!isTauriRuntime()) {
    return makeBrowserFallbackSnapshot();
  }
  return invoke<DashboardSnapshot>("get_dashboard_snapshot", {
    forceSleepRefresh,
  });
}

export async function toggleKeepAwake(): Promise<KeepAwakeState> {
  if (!isTauriRuntime()) {
    browserKeepAwake = {
      ...browserKeepAwake,
      active: !browserKeepAwake.active,
      remainingSecs: !browserKeepAwake.active ? 3600 : null,
    };
    return browserKeepAwake;
  }
  return invoke<KeepAwakeState>("toggle_keep_awake");
}

export async function setKeepAwake(config: KeepAwakeConfig): Promise<KeepAwakeState> {
  if (!isTauriRuntime()) {
    browserKeepAwake = {
      ...browserKeepAwake,
      active: config.enabled,
      mode: config.mode,
      remainingSecs: config.enabled && config.durationMins ? config.durationMins * 60 : null,
      autoOffBelowBatteryPct: config.autoOffBelowBatteryPct,
      autoOffOnUnplug: config.autoOffOnUnplug,
      watchAppName: config.watchAppName,
    };
    return browserKeepAwake;
  }
  return invoke<KeepAwakeState>("set_keep_awake", { config });
}

export async function setDemoMode(enabled: boolean): Promise<boolean> {
  if (!isTauriRuntime()) return enabled;
  return invoke<boolean>("set_demo_mode", { enabled });
}

export async function quitProcess(pid: number, appName?: string | null): Promise<ActionResult> {
  if (!isTauriRuntime()) {
    return { ok: true, message: `[preview] Sent graceful Quit to ${appName ?? `PID ${pid}`}` };
  }
  return invoke<ActionResult>("quit_process", { pid, appName: appName ?? null });
}

export async function forceKillProcess(pid: number, confirmed: boolean): Promise<ActionResult> {
  if (!isTauriRuntime()) {
    return { ok: true, message: `[preview] Force-killed PID ${pid}` };
  }
  return invoke<ActionResult>("force_kill_process", { pid, confirmed });
}

export async function hideDashboard(): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("hide_dashboard");
}
