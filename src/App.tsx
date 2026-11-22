import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import {
  fetchSnapshot,
  forceKillProcess,
  hideDashboard,
  quitProcess,
  setKeepAwake,
  toggleKeepAwake,
} from "./api";
import { MiniSparkline } from "./components/MiniSparkline";
import {
  ReactiveWaveform,
  type LiveMetricMode,
  type RailSegment,
  type TimedPoint,
} from "./components/ReactiveWaveform";
import { SleepChart } from "./components/SleepChart";
import { formatBytes, formatDuration, formatEta } from "./format";
import type { DashboardSnapshot, KeepAwakeConfig } from "./types";

type TopView = "live" | "sleep";

const MAX_LIVE_PTS = 40;

export function App() {
  const [snap, setSnap] = useState<DashboardSnapshot | null>(null);
  const [view, setView] = useState<TopView>("live");
  const [metric, setMetric] = useState<LiveMetricMode>("power");
  const [awakeDrawerOpen, setAwakeDrawerOpen] = useState(false);
  // Track selection by stable app group ID (never row index!) so 1.5s re-sorting never shifts targets.
  const [selectedAppId, setSelectedAppId] = useState<string | null>(null);
  const [expandedAppId, setExpandedAppId] = useState<string | null>(null);
  const [expandedSessionId, setExpandedSessionId] = useState<string | null>(null);
  const [showSystemBlockers, setShowSystemBlockers] = useState(false);
  const [filterOpen, setFilterOpen] = useState(false);
  const [filterText, setFilterText] = useState("");
  const [confirmKillPid, setConfirmKillPid] = useState<number | null>(null);
  const [copiedId, setCopiedId] = useState<string | null>(null);
  const [toast, setToast] = useState<{ text: string; err?: boolean } | null>(null);

  // Honest timestamped rolling live observations (no fake sine wave or mixed 60s SQLite intervals)
  const [powerSeries, setPowerSeries] = useState<TimedPoint[]>([]);
  const [cpuSeries, setCpuSeries] = useState<TimedPoint[]>([]);
  const [memSeries, setMemSeries] = useState<TimedPoint[]>([]);

  // Single in-flight guard so slow polls never stack
  const inFlightRef = useRef(false);
  const viewRef = useRef<TopView>(view);
  viewRef.current = view;

  const liveTabRef = useRef<HTMLButtonElement>(null);
  const sleepTabRef = useRef<HTMLButtonElement>(null);
  const thumbRef = useRef<HTMLDivElement>(null);
  const filterInputRef = useRef<HTMLInputElement>(null);

  const showToast = useCallback((text: string, err = false) => {
    setToast({ text, err });
  }, []);

  useEffect(() => {
    if (!toast) return;
    const t = setTimeout(() => setToast(null), 3200);
    return () => clearTimeout(t);
  }, [toast]);

  useEffect(() => {
    if (confirmKillPid === null) return;
    const t = setTimeout(() => setConfirmKillPid(null), 4000);
    return () => clearTimeout(t);
  }, [confirmKillPid]);

  useEffect(() => {
    if (!copiedId) return;
    const t = setTimeout(() => setCopiedId(null), 1800);
    return () => clearTimeout(t);
  }, [copiedId]);

  const refresh = useCallback(
    async (includeSleep?: boolean, forceSleep = false) => {
      if (inFlightRef.current) return;
      inFlightRef.current = true;
      try {
        const wantSleep = includeSleep ?? viewRef.current === "sleep";
        const next = await fetchSnapshot(wantSleep, forceSleep);
        setSnap(next);

        const ts = next.timestampMs || Date.now();
        if (next.power.systemWatts !== null) {
          const w = next.power.systemWatts;
          setPowerSeries((prev) => [...prev.slice(-(MAX_LIVE_PTS - 1)), { ts, val: w }]);
        }

        const cpuVal = next.power.components?.available
          ? next.power.components.cpuActivePct
          : Math.min(100, next.processes.totalCpuPct / 4);
        setCpuSeries((prev) => [...prev.slice(-(MAX_LIVE_PTS - 1)), { ts, val: cpuVal }]);

        const memGb = next.processes.memoryPressure.usedBytes / 1_073_741_824;
        setMemSeries((prev) => [...prev.slice(-(MAX_LIVE_PTS - 1)), { ts, val: memGb }]);
      } catch (e) {
        showToast(`telemetry error: ${String(e)}`, true);
      } finally {
        inFlightRef.current = false;
      }
    },
    [showToast],
  );

  // Live 1.5s polling on `live` (skipping `pmset -g log`); slow 10s polling on `sleep`.
  useEffect(() => {
    void refresh(view === "sleep", false);
    const cadenceMs = view === "sleep" ? 10_000 : 1500;
    const id = setInterval(() => {
      if (document.visibilityState === "visible") {
        void refresh(view === "sleep", false);
      }
    }, cadenceMs);
    return () => clearInterval(id);
  }, [refresh, view]);

  const syncThumb = useCallback(() => {
    const btn = view === "live" ? liveTabRef.current : sleepTabRef.current;
    const thumb = thumbRef.current;
    if (!btn || !thumb) return;
    thumb.style.width = `${btn.offsetWidth}px`;
    thumb.style.transform = `translateX(${btn.offsetLeft - 2}px)`;
  }, [view]);

  useLayoutEffect(() => {
    syncThumb();
  }, [syncThumb, snap?.sleep.sessions.length]);

  useEffect(() => {
    window.addEventListener("resize", syncThumb);
    return () => window.removeEventListener("resize", syncThumb);
  }, [syncThumb]);

  const query = filterText.trim().toLowerCase();

  const visibleApps = useMemo(() => {
    if (!snap) return [];
    const filtered = snap.processes.groups.filter((g) => {
      if (!query) return true;
      return `${g.name} ${g.mostlyIn ?? ""} ${g.verdict}`.toLowerCase().includes(query);
    });
    return [...filtered].sort((a, b) => {
      if (metric === "cpu") return b.cpuPct - a.cpuPct;
      if (metric === "mem") return b.footprintBytes - a.footprintBytes;
      const scoreA = (a.isBlockingSleep ? 25 : 0) + a.energyImpact;
      const scoreB = (b.isBlockingSleep ? 25 : 0) + b.energyImpact;
      return scoreB - scoreA || b.footprintBytes - a.footprintBytes;
    });
  }, [snap, query, metric]);

  // Keep selectedAppId stable across re-sorts; only fall back if that app vanished.
  const selectedApp = useMemo(() => {
    if (visibleApps.length === 0) return null;
    if (selectedAppId) {
      const found = visibleApps.find((a) => a.id === selectedAppId);
      if (found) return found;
    }
    return visibleApps[0];
  }, [visibleApps, selectedAppId]);

  const handleToggleAwake = useCallback(async () => {
    try {
      const ka = await toggleKeepAwake();
      showToast(
        ka.active
          ? `keep-awake ON (${ka.mode}${ka.remainingSecs ? ` · ${formatDuration(ka.remainingSecs)}` : ""})`
          : ka.lastAutoReleaseReason ?? "keep-awake OFF",
      );
      void refresh(false, false);
    } catch (e) {
      showToast(String(e), true);
    }
  }, [refresh, showToast]);

  const handleUpdateKeepAwake = useCallback(
    async (patch: Partial<KeepAwakeConfig>) => {
      if (!snap) return;
      const ka = snap.keepAwake;
      const cfg: KeepAwakeConfig = {
        enabled: patch.enabled ?? ka.active,
        mode: (patch.mode ?? (ka.mode === "system" ? "system" : "display")) as
          | "display"
          | "system",
        durationMins:
          patch.durationMins !== undefined
            ? patch.durationMins
            : ka.remainingSecs !== null
              ? Math.max(1, Math.round(ka.remainingSecs / 60))
              : 60,
        autoOffBelowBatteryPct:
          patch.autoOffBelowBatteryPct !== undefined
            ? patch.autoOffBelowBatteryPct
            : ka.autoOffBelowBatteryPct,
        autoOffOnUnplug: patch.autoOffOnUnplug ?? ka.autoOffOnUnplug,
        watchAppName: patch.watchAppName !== undefined ? patch.watchAppName : ka.watchAppName,
      };
      try {
        const next = await setKeepAwake(cfg);
        showToast(
          next.lastAutoReleaseReason ??
            (next.active ? `keep-awake updated (${next.mode})` : "keep-awake OFF"),
          !next.active && Boolean(next.lastAutoReleaseReason),
        );
        void refresh(false, false);
      } catch (e) {
        showToast(String(e), true);
      }
    },
    [snap, refresh, showToast],
  );

  const handleQuit = useCallback(
    async (pid: number, startTimeSecs: number, name: string, protectedSys: boolean) => {
      if (protectedSys) {
        showToast(`protected macOS system process: ${name}`, true);
        return;
      }
      const res = await quitProcess(pid, startTimeSecs, name);
      showToast(res.message, !res.ok);
      setTimeout(() => void refresh(false, false), 350);
    },
    [refresh, showToast],
  );

  const handleKill = useCallback(
    async (pid: number, startTimeSecs: number, name: string, protectedSys: boolean) => {
      if (protectedSys) {
        showToast(`protected macOS system process: ${name}`, true);
        return;
      }
      if (confirmKillPid !== pid) {
        setConfirmKillPid(pid);
        showToast(`click Confirm Kill again to SIGKILL ${name} (pid ${pid})`, true);
        return;
      }
      setConfirmKillPid(null);
      const res = await forceKillProcess(pid, startTimeSecs, true);
      showToast(res.message, !res.ok);
      setTimeout(() => void refresh(false, false), 350);
    },
    [confirmKillPid, refresh, showToast],
  );

  const handleCopyFix = useCallback(
    async (id: string, cmd: string) => {
      try {
        await navigator.clipboard.writeText(cmd);
        setCopiedId(id);
        showToast(`copied: ${cmd}`);
      } catch {
        showToast(cmd);
      }
    },
    [showToast],
  );

  // Keyboard shortcuts (respects focused buttons/inputs, ignores OS key-repeat on destructive keys!)
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      const targetTag = (e.target as HTMLElement | null)?.tagName;
      const inFormControl =
        targetTag === "INPUT" || targetTag === "SELECT" || targetTag === "TEXTAREA";

      if (filterOpen || inFormControl) {
        if (e.key === "Escape" || e.key === "Enter") {
          e.preventDefault();
          setFilterOpen(false);
          if (e.key === "Escape") setFilterText("");
          (e.target as HTMLElement | null)?.blur();
        }
        return;
      }

      if (e.metaKey || e.ctrlKey || e.altKey) return;

      // Never steal Enter/Space from a focused button!
      if ((e.key === "Enter" || e.key === " ") && targetTag === "BUTTON") {
        return;
      }

      if (e.key === "Escape") {
        e.preventDefault();
        if (confirmKillPid !== null) return setConfirmKillPid(null);
        if (awakeDrawerOpen) return setAwakeDrawerOpen(false);
        if (expandedAppId) return setExpandedAppId(null);
        if (filterText) return setFilterText("");
        void hideDashboard();
        return;
      }

      if (e.key === "1") return setView("live");
      if (e.key === "2") return setView("sleep");
      if (e.key === "p") {
        setView("live");
        return setMetric("power");
      }
      if (e.key === "u") {
        setView("live");
        return setMetric("cpu");
      }
      if (e.key === "m") {
        setView("live");
        return setMetric("mem");
      }
      if (e.key === "c" && !e.repeat) {
        e.preventDefault();
        return void handleToggleAwake();
      }
      if (e.key === "/") {
        e.preventDefault();
        setView("live");
        setFilterOpen(true);
        setTimeout(() => filterInputRef.current?.focus(), 0);
        return;
      }

      if (view === "live") {
        const curIdx = selectedApp ? visibleApps.findIndex((a) => a.id === selectedApp.id) : -1;
        if (e.key === "ArrowDown" || e.key === "j") {
          e.preventDefault();
          setConfirmKillPid(null);
          if (visibleApps.length > 0) {
            const nextIdx = Math.min(visibleApps.length - 1, Math.max(0, curIdx + 1));
            setSelectedAppId(visibleApps[nextIdx].id);
          }
        } else if (e.key === "ArrowUp" || e.key === "k") {
          e.preventDefault();
          setConfirmKillPid(null);
          if (visibleApps.length > 0) {
            const prevIdx = Math.max(0, curIdx <= 0 ? 0 : curIdx - 1);
            setSelectedAppId(visibleApps[prevIdx].id);
          }
        } else if (e.key === "ArrowRight" || e.key === "Enter") {
          if (selectedApp) {
            e.preventDefault();
            setExpandedAppId((cur) => (cur === selectedApp.id ? null : selectedApp.id));
          }
        } else if (e.key === "ArrowLeft") {
          setExpandedAppId(null);
        } else if (e.key === "q" && !e.repeat) {
          if (selectedApp) {
            e.preventDefault();
            void handleQuit(
              selectedApp.primaryPid,
              selectedApp.primaryStartTimeSecs,
              selectedApp.name,
              selectedApp.isSystemProtected,
            );
          }
        }
      } else if (view === "sleep" && e.key === "y" && !e.repeat && snap) {
        const firstFix = snap.audit.find((a) => a.severity !== "ok" && a.fixCommand);
        if (firstFix?.fixCommand) {
          e.preventDefault();
          void handleCopyFix(firstFix.id, firstFix.fixCommand);
        }
      }
    };

    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [
    filterOpen,
    confirmKillPid,
    awakeDrawerOpen,
    expandedAppId,
    filterText,
    view,
    visibleApps,
    selectedApp,
    snap,
    handleToggleAwake,
    handleQuit,
    handleCopyFix,
  ]);

  if (!snap) {
    return (
      <div className="app-shell" style={{ alignItems: "center", justifyContent: "center" }}>
        <span style={{ color: "#7C8596" }}>reading Apple Silicon telemetry...</span>
      </div>
    );
  }

  const { power, sleep, blockers, keepAwake, processes, audit } = snap;
  const batt = power.battery;
  const comp = power.components;
  const mp = processes.memoryPressure;
  const userBlockers = blockers.filter((b) => !b.isSystemProtected && !b.isSelfWattlog);
  const auditFixes = audit.filter((a) => a.severity !== "ok");

  const totalGb = Math.max(1, mp.totalBytes / 1_073_741_824);
  const usedGb = mp.usedBytes / 1_073_741_824;
  const cpuDisplayPct = comp?.available
    ? comp.cpuActivePct
    : Math.min(100, processes.totalCpuPct / 4);
  const gpuDisplayPct = comp?.available ? comp.gpuActivePct : 0;

  const powerVals = powerSeries.map((p) => p.val);
  const powerPeak =
    powerVals.length > 0 ? Math.max(...powerVals) : power.systemWatts ?? 0;
  const powerAvg =
    powerVals.length > 0
      ? powerVals.reduce((a, b) => a + b, 0) / powerVals.length
      : power.systemWatts ?? 0;

  const powerRail: RailSegment[] = (() => {
    if (power.systemWatts === null) {
      return [{ label: "Power sensors unavailable", pct: 100, color: "#334155" }];
    }
    const t = Math.max(power.systemWatts, 0.1);
    if (comp?.available) {
      const items: RailSegment[] = [
        { label: `CPU ${comp.cpuWatts.toFixed(1)}W`, pct: (comp.cpuWatts / t) * 100, color: "#38BDF8" },
        { label: `GPU ${comp.gpuWatts.toFixed(1)}W`, pct: (comp.gpuWatts / t) * 100, color: "#A855F7" },
        {
          label: `ANE+RAM ${(comp.aneWatts + comp.dramWatts).toFixed(1)}W`,
          pct: ((comp.aneWatts + comp.dramWatts) / t) * 100,
          color: "#22C55E",
        },
        {
          label: `Display/Sys ${comp.displayOtherWatts.toFixed(1)}W`,
          pct: (comp.displayOtherWatts / t) * 100,
          color: "#64748B",
        },
      ];
      return items.filter((i) => i.pct > 0.5);
    }
    return [
      {
        label: `${power.sourceTier} · ${power.systemWatts.toFixed(1)}W`,
        pct: 100,
        color: "#38BDF8",
      },
    ];
  })();

  const cpuRail: RailSegment[] = [
    { label: `CPU ${cpuDisplayPct.toFixed(0)}%`, pct: Math.max(4, cpuDisplayPct), color: "#FBBF24" },
    { label: `GPU ${gpuDisplayPct.toFixed(0)}%`, pct: Math.max(2, gpuDisplayPct), color: "#A855F7" },
    {
      label: `Headroom ${Math.max(0, 100 - cpuDisplayPct).toFixed(0)}%`,
      pct: Math.max(4, 100 - cpuDisplayPct),
      color: "#1E293B",
    },
  ];

  const memRail: RailSegment[] = (() => {
    const appB = Math.max(0, mp.usedBytes - mp.wiredBytes - mp.compressedBytes);
    const freeB = Math.max(0, mp.totalBytes - mp.usedBytes);
    const denom = Math.max(1, mp.totalBytes);
    return [
      { label: `App ${formatBytes(appB)}`, pct: (appB / denom) * 100, color: "#22C55E" },
      { label: `Wired ${formatBytes(mp.wiredBytes)}`, pct: (mp.wiredBytes / denom) * 100, color: "#38BDF8" },
      {
        label: `Compressed ${formatBytes(mp.compressedBytes)}`,
        pct: (mp.compressedBytes / denom) * 100,
        color: "#FBBF24",
      },
      { label: `Free ${formatBytes(freeB)}`, pct: (freeB / denom) * 100, color: "#1E293B" },
    ].filter((r) => r.pct > 0.5);
  })();

  const liveSpanSecs =
    powerSeries.length >= 2
      ? Math.max(1, Math.round((powerSeries[powerSeries.length - 1].ts - powerSeries[0].ts) / 1000))
      : 0;

  const waveformConfig =
    metric === "power"
      ? {
          points: powerSeries,
          maxVal: 24,
          unit: "W",
          title:
            liveSpanSecs > 0
              ? `POWER DRAW · LIVE (${liveSpanSecs}S)`
              : "POWER DRAW · COLLECTING LIVE SAMPLES",
          rightMeta:
            power.systemWatts !== null ? (
              <>
                peak <strong style={{ color: "#FB923C" }}>{powerPeak.toFixed(1)}W</strong> · avg{" "}
                {powerAvg.toFixed(1)}W
              </>
            ) : (
              "sensor unavailable"
            ),
          rail: powerRail,
        }
      : metric === "cpu"
        ? {
            points: cpuSeries,
            maxVal: 100,
            unit: "%",
            title:
              liveSpanSecs > 0
                ? `CPU & GPU ACTIVITY · LIVE (${liveSpanSecs}S)`
                : "CPU & GPU ACTIVITY · LIVE",
            rightMeta: (
              <>
                {comp?.cpuTempC ? (
                  <>
                    cpu <strong style={{ color: "#FBBF24" }}>{comp.cpuTempC.toFixed(0)}°C</strong>
                  </>
                ) : (
                  <>cpu {cpuDisplayPct.toFixed(0)}%</>
                )}
                {comp?.gpuTempC ? ` · gpu ${comp.gpuTempC.toFixed(0)}°C` : ""}
                {comp?.fanRpm !== null && comp?.fanRpm !== undefined
                  ? ` · ${comp.fanRpm} RPM`
                  : ""}
              </>
            ),
            rail: cpuRail,
          }
        : {
            points: memSeries,
            maxVal: totalGb,
            unit: " GB",
            title: `MEMORY FOOTPRINT & PRESSURE · ${usedGb.toFixed(1)} / ${totalGb.toFixed(0)} GB`,
            rightMeta: (
              <>
                pressure{" "}
                <strong
                  style={{
                    color:
                      mp.level === "red"
                        ? "#FB7185"
                        : mp.level === "yellow"
                          ? "#FBBF24"
                          : "#4ADE80",
                  }}
                >
                  {mp.level.toUpperCase()}
                </strong>{" "}
                · swap {formatBytes(mp.swapUsedBytes)}
              </>
            ),
            rail: memRail,
          };

  // Explicit, period-consistent Sleep Health Verdict (NO false green on empty/unscoped data!)
  const unplug = sleep.sinceUnplug;
  const scopedSessions = sleep.sessions.filter((s) => s.inCurrentUnplug);
  const scopedWorst =
    scopedSessions.find((s) => s.severity !== "ok") ?? scopedSessions[0] ?? null;
  const scopedReasons =
    sleep.sinceUnplugWakeReasons.length > 0
      ? sleep.sinceUnplugWakeReasons
      : sleep.topWakeReasons;

  const hasSleepWarning =
    (scopedWorst !== null && (scopedWorst.drainPct >= 5 || scopedWorst.darkWakeCount >= 6)) ||
    auditFixes.length > 0;

  const sleepVerdict: { tone: "ok" | "warn" | "neutral"; title: string; badge: string; desc: string } =
    (() => {
      if (unplug.dataStatus === "log_unavailable" || !sleep.rawLogAvailable) {
        return {
          tone: "neutral",
          title: "Sleep History Not Loaded",
          badge: "insufficient history",
          desc: "macOS power log data wasn't available yet. Click Refresh to scan recent sleep sessions.",
        };
      }
      if (unplug.dataStatus === "on_ac") {
        const recentBatt = sleep.sessions[0];
        return {
          tone: "neutral",
          title: "Plugged Into AC Power",
          badge: "on ac",
          desc: recentBatt
            ? `Most recent battery sleep (${recentBatt.startTime} → ${recentBatt.endTime}) lost ${recentBatt.drainPct}% with ${recentBatt.darkWakeCount} silent wakes.`
            : "Unplug to start tracking a fresh battery discharge and sleep window.",
        };
      }
      if (unplug.dataStatus === "insufficient_history") {
        return {
          tone: "neutral",
          title: "Insufficient Unplug History",
          badge: "collecting",
          desc: `Wattlog has ${formatDuration(unplug.historyCoverageSecs)} of coverage and hasn't observed the last AC unplug yet. Recent 7-day battery sleep sessions are shown below.`,
        };
      }
      if (unplug.dataStatus === "no_battery_sleep" || !scopedWorst) {
        return {
          tone: "neutral",
          title: "No Battery Sleep Since Unplug",
          badge: `since ${unplug.startedAt ?? "unplug"}`,
          desc: `Your Mac has been awake since unplugging (${unplug.totalDropPct}% used actively). Once you sleep on battery, overnight drain and silent wake causes will appear here.`,
        };
      }
      if (scopedWorst.drainPct >= 8 || scopedWorst.darkWakeCount >= 10) {
        const top = scopedWorst.worstWakeReason;
        return {
          tone: "warn",
          title: `Restless Sleep (${scopedWorst.startTime} → ${scopedWorst.endTime})`,
          badge: `-${scopedWorst.drainPct}% asleep`,
          desc: `Your Mac woke silently ${scopedWorst.darkWakeCount} times during ${formatDuration(scopedWorst.durationSecs)} of sleep${top ? ` — mostly from ${top.label}` : ""}. Healthy sleep should lose under 3% overnight.`,
        };
      }
      if (scopedWorst.drainPct >= 3 || scopedWorst.darkWakeCount >= 3) {
        const top = scopedWorst.worstWakeReason;
        return {
          tone: "warn",
          title: `Light Background Wakes (${scopedWorst.startTime} → ${scopedWorst.endTime})`,
          badge: `${scopedWorst.darkWakeCount} silent wakes`,
          desc: `Lost ${scopedWorst.drainPct}% over ${formatDuration(scopedWorst.durationSecs)}${top ? `, triggered mainly by ${top.label}` : ""}.`,
        };
      }
      return {
        tone: "ok",
        title: `Healthy Sleep (${scopedWorst.startTime} → ${scopedWorst.endTime})`,
        badge: `-${scopedWorst.drainPct}% asleep`,
        desc: `Stayed cleanly in deep sleep (${scopedWorst.darkWakeCount} silent wakes over ${formatDuration(scopedWorst.durationSecs)}).`,
      };
    })();

  const etaStr = batt ? formatEta(batt.timeRemainingMins) : "";
  const stateTxt =
    power.state === "charging"
      ? "charging"
      : power.state === "full"
        ? "charged"
        : power.state === "acOnly"
          ? "ac power"
          : "discharging";

  return (
    <div className="app-shell">
      {/* Top Bar */}
      <header className="topbar" data-tauri-drag-region>
        <div className="tb-left" data-tauri-drag-region>
          <span className="brand">
            <span className="brand-pulse" />
            wattlog
          </span>
        </div>

        <div className="glide-nav">
          <div ref={thumbRef} className="glide-thumb" />
          <button
            ref={liveTabRef}
            className={`glide-btn ${view === "live" ? "active" : ""}`}
            onClick={() => setView("live")}
          >
            live
          </button>
          <button
            ref={sleepTabRef}
            className={`glide-btn ${view === "sleep" ? "active" : ""}`}
            onClick={() => setView("sleep")}
          >
            sleep &amp; battery
            {hasSleepWarning && <span className="alert-dot" />}
          </button>
        </div>

        <div className="tb-right">
          <div className={`awake-ctrl ${keepAwake.active ? "on" : ""}`}>
            <button
              className="awake-btn"
              onClick={handleToggleAwake}
              title="Toggle Keep-Awake (c)"
            >
              <span className="awake-led" />
              <span>
                {keepAwake.active
                  ? keepAwake.remainingSecs !== null
                    ? `awake · ${formatDuration(keepAwake.remainingSecs)}`
                    : "awake · ON"
                  : "awake: off"}
              </span>
            </button>
            <button
              className="awake-caret"
              onClick={() => setAwakeDrawerOpen((v) => !v)}
              title="Keep-Awake timer & safety guardrails"
              aria-expanded={awakeDrawerOpen}
            >
              {awakeDrawerOpen ? "▴" : "▾"}
            </button>
          </div>
        </div>
      </header>

      {/* Smooth Unfolding Keep-Awake Drawer (unmounts interactive controls when closed so Tab never reaches them!) */}
      <div className={`drawer-wrap ${awakeDrawerOpen ? "open" : ""}`}>
        <div className="drawer-inner">
          {awakeDrawerOpen && (
            <div className="drawer-content">
              <div className="d-group">
                <span className="d-lbl">timer</span>
                {[
                  { label: "30m", val: 30 },
                  { label: "1h", val: 60 },
                  { label: "2h", val: 120 },
                  { label: "∞", val: null },
                ].map((opt) => (
                  <button
                    key={opt.label}
                    className="d-chip"
                    onClick={() =>
                      handleUpdateKeepAwake({ enabled: true, durationMins: opt.val })
                    }
                  >
                    {opt.label}
                  </button>
                ))}
              </div>

              <div className="d-group">
                <span className="d-lbl">scope</span>
                {(["display", "system"] as const).map((m) => (
                  <button
                    key={m}
                    className={`d-chip ${keepAwake.mode === m ? "on" : ""}`}
                    onClick={() => handleUpdateKeepAwake({ mode: m })}
                  >
                    {m}
                  </button>
                ))}
              </div>

              <div className="d-group">
                <span className="d-lbl">safety</span>
                <button
                  className={`d-chip ${keepAwake.autoOffBelowBatteryPct ? "safe" : ""}`}
                  onClick={() =>
                    handleUpdateKeepAwake({
                      autoOffBelowBatteryPct: keepAwake.autoOffBelowBatteryPct ? null : 20,
                    })
                  }
                >
                  auto-off &lt;{keepAwake.autoOffBelowBatteryPct ?? 20}%{" "}
                  {keepAwake.autoOffBelowBatteryPct ? "ON" : "off"}
                </button>
                <button
                  className={`d-chip ${keepAwake.autoOffOnUnplug ? "on" : ""}`}
                  onClick={() =>
                    handleUpdateKeepAwake({ autoOffOnUnplug: !keepAwake.autoOffOnUnplug })
                  }
                >
                  off on unplug
                </button>
              </div>

              <div className="d-group">
                <span className="d-lbl">app</span>
                <select
                  className="d-select"
                  value={keepAwake.watchAppName ?? ""}
                  onChange={(e) =>
                    handleUpdateKeepAwake({ watchAppName: e.target.value || null })
                  }
                >
                  <option value="">manual</option>
                  {processes.groups
                    .filter((g) => !g.isSystemProtected)
                    .slice(0, 10)
                    .map((g) => (
                      <option key={g.id} value={g.name}>
                        while {g.name} runs
                      </option>
                    ))}
                </select>
              </div>
            </div>
          )}
        </div>
      </div>

      {/* Main Content Stage */}
      <main className="stage">
        {view === "live" ? (
          <div key="live" className="view-pane">
            {/* 3-Metric Interactive Strip: Power · CPU & GPU · Memory */}
            <div className="tri-strip">
              <button
                className={`m-tile ${metric === "power" ? "active" : ""}`}
                style={
                  {
                    "--tile-accent": "rgba(56, 189, 248, 0.45)",
                    "--tile-line": "#38BDF8",
                  } as React.CSSProperties
                }
                onClick={() => setMetric("power")}
              >
                <div className="mt-top">
                  <span className="mt-lbl">Power</span>
                  <MiniSparkline points={powerVals} maxVal={24} color="#38BDF8" />
                </div>
                <div className="mt-val-row">
                  <span className="mt-val">
                    {power.systemWatts !== null ? power.systemWatts.toFixed(1) : "—"}
                    <span className="mt-unit">W</span>
                  </span>
                  <span
                    style={{
                      fontSize: "10px",
                      color:
                        (power.systemWatts ?? 0) >= 16
                          ? "#FB923C"
                          : power.state === "charging"
                            ? "#4ADE80"
                            : "#38BDF8",
                    }}
                  >
                    {etaStr || stateTxt}
                  </span>
                </div>
                <div className="mt-sub">
                  {batt
                    ? `${batt.chargePct}% · ${stateTxt} · ${Math.round(batt.healthPct)}% health`
                    : power.sourceTier}
                </div>
              </button>

              <button
                className={`m-tile ${metric === "cpu" ? "active" : ""}`}
                style={
                  {
                    "--tile-accent": "rgba(251, 191, 36, 0.45)",
                    "--tile-line": "#FBBF24",
                  } as React.CSSProperties
                }
                onClick={() => setMetric("cpu")}
              >
                <div className="mt-top">
                  <span className="mt-lbl">CPU &amp; GPU</span>
                  <MiniSparkline
                    points={cpuSeries.map((p) => p.val)}
                    maxVal={100}
                    color="#FBBF24"
                  />
                </div>
                <div className="mt-val-row">
                  <span className="mt-val">
                    {Math.round(cpuDisplayPct)}
                    <span className="mt-unit">%</span>
                  </span>
                  <span style={{ fontSize: "10px", color: "#FBBF24" }}>
                    {comp?.cpuTempC
                      ? `${comp.cpuTempC.toFixed(0)}°C`
                      : `${processes.totalCpuPct.toFixed(0)}% sum`}
                  </span>
                </div>
                <div className="mt-sub">
                  GPU {Math.round(gpuDisplayPct)}%
                  {comp?.gpuTempC ? ` · ${comp.gpuTempC.toFixed(0)}°C` : ""}
                  {comp?.fanRpm !== null && comp?.fanRpm !== undefined
                    ? ` · ${comp.fanRpm} RPM`
                    : ""}
                </div>
              </button>

              <button
                className={`m-tile ${metric === "mem" ? "active" : ""}`}
                style={
                  {
                    "--tile-accent":
                      mp.level === "red"
                        ? "rgba(239, 68, 68, 0.5)"
                        : mp.level === "yellow"
                          ? "rgba(251, 191, 36, 0.45)"
                          : "rgba(34, 197, 94, 0.45)",
                    "--tile-line":
                      mp.level === "red"
                        ? "#EF4444"
                        : mp.level === "yellow"
                          ? "#FBBF24"
                          : "#22C55E",
                  } as React.CSSProperties
                }
                onClick={() => setMetric("mem")}
              >
                <div className="mt-top">
                  <span className="mt-lbl">Memory</span>
                  <svg className="mt-spark" viewBox="0 0 48 15" aria-hidden="true">
                    <rect x="0" y="5" width="48" height="4" rx="2" fill="rgba(255,255,255,0.08)" />
                    <rect
                      x="0"
                      y="5"
                      width={Math.max(4, Math.min(48, (usedGb / totalGb) * 48))}
                      height="4"
                      rx="2"
                      fill={
                        mp.level === "red"
                          ? "#EF4444"
                          : mp.level === "yellow"
                            ? "#FBBF24"
                            : "#22C55E"
                      }
                    />
                  </svg>
                </div>
                <div className="mt-val-row">
                  <span className="mt-val">
                    {usedGb.toFixed(1)}
                    <span className="mt-unit">/{totalGb.toFixed(0)}G</span>
                  </span>
                  <span
                    style={{
                      fontSize: "10px",
                      color:
                        mp.level === "red"
                          ? "#FB7185"
                          : mp.level === "yellow"
                            ? "#FBBF24"
                            : "#4ADE80",
                    }}
                  >
                    {mp.level === "green" ? "normal" : mp.level}
                  </span>
                </div>
                <div className="mt-sub">
                  compressed {formatBytes(mp.compressedBytes)} · swap{" "}
                  {formatBytes(mp.swapUsedBytes)}
                </div>
              </button>
            </div>

            {/* Unified Load-Reactive OLED Waveform + Hardware Rail */}
            <ReactiveWaveform
              mode={metric}
              points={waveformConfig.points}
              maxVal={waveformConfig.maxVal}
              unit={waveformConfig.unit}
              title={waveformConfig.title}
              rightMeta={waveformConfig.rightMeta}
              rail={waveformConfig.rail}
            />

            {/* Pinned Inline Sleep Blockers Strip */}
            {userBlockers.length > 0 && (
              <div className="blocker-strip">
                <div className="blocker-left">
                  <span className="b-beacon" />
                  <span className="blocker-text">
                    <strong>{userBlockers[0].appName}</strong>{" "}
                    <span style={{ color: "#9AA3B5" }}>
                      blocking sleep ({formatDuration(userBlockers[0].durationSecs)})
                      {userBlockers.length > 1 ? ` · +${userBlockers.length - 1} more` : ""}
                    </span>
                  </span>
                </div>
                <div className="blocker-actions">
                  {userBlockers.slice(0, 2).map((b) => (
                    <button
                      key={`${b.pid}-${b.assertionType}`}
                      className="btn red"
                      onClick={() =>
                        handleQuit(b.pid, b.startTimeSecs, b.appName, b.isSystemProtected)
                      }
                      title={`${b.assertionType}: ${b.assertionName}`}
                    >
                      Quit {b.appName}
                    </button>
                  ))}
                  {blockers.length > userBlockers.length && (
                    <button
                      className="btn"
                      onClick={() => setShowSystemBlockers((v) => !v)}
                    >
                      {showSystemBlockers ? "Hide all" : `All (${blockers.length})`}
                    </button>
                  )}
                </div>
              </div>
            )}

            {showSystemBlockers && blockers.length > 0 && (
              <div className="graph-well" style={{ marginBottom: 10 }}>
                <div className="gw-head">
                  <span>ACTIVE MACOS SLEEP ASSERTIONS ({blockers.length})</span>
                  <button className="btn" onClick={() => setShowSystemBlockers(false)}>
                    Close
                  </button>
                </div>
                {blockers.map((b, idx) => (
                  <div key={`${b.pid}-${idx}`} className="sub-item">
                    <span className="sub-item-left">
                      <strong style={{ color: "#EAECEF" }}>{b.appName}</strong>{" "}
                      <span style={{ color: "#677082" }}>
                        (pid {b.pid}) · {b.assertionType} · {b.assertionName}
                      </span>
                    </span>
                    <span className="sub-item-right">
                      <span>{formatDuration(b.durationSecs)}</span>
                      {!b.isSystemProtected && !b.isSelfWattlog && (
                        <button
                          className="btn red"
                          onClick={() =>
                            handleQuit(b.pid, b.startTimeSecs, b.appName, b.isSystemProtected)
                          }
                        >
                          Quit
                        </button>
                      )}
                    </span>
                  </div>
                ))}
              </div>
            )}

            {processes.kernelThermalNote && (
              <div className="thermal-strip">└ {processes.kernelThermalNote}</div>
            )}

            {filterOpen && (
              <div className="filter-row">
                <span>/</span>
                <input
                  ref={filterInputRef}
                  className="filter-input"
                  value={filterText}
                  onChange={(e) => setFilterText(e.target.value)}
                  placeholder="filter apps (Enter to keep, Esc to clear)..."
                />
              </div>
            )}

            {/* Auto-Pivoting Grouped Apps Table */}
            <div className="sec-bar">
              <span>
                top apps · sorted by{" "}
                {metric === "power"
                  ? "energy impact"
                  : metric === "cpu"
                    ? "cpu %"
                    : "memory footprint"}
              </span>
              <div className="sec-cols">
                <span style={{ minWidth: 52, textAlign: "right" }}>energy</span>
                <span style={{ minWidth: 52, textAlign: "right" }}>cpu</span>
                <span style={{ minWidth: 52, textAlign: "right" }}>memory</span>
                <span style={{ minWidth: 78, textAlign: "right" }}>action</span>
              </div>
            </div>

            {visibleApps.map((g) => {
              const isSel = selectedApp?.id === g.id;
              const isExp = expandedAppId === g.id;

              return (
                <div
                  key={g.id}
                  className={`app-wrap ${isSel ? "selected" : ""} ${isExp ? "exp" : ""}`}
                >
                  <div
                    className="app-row"
                    onClick={() => {
                      setSelectedAppId(g.id);
                      setExpandedAppId(isExp ? null : g.id);
                    }}
                  >
                    <div className="ar-left">
                      <div className="ar-top">
                        <span className="chev">▸</span>
                        <span className="ar-name">{g.name}</span>
                        {g.helperCount > 1 && (
                          <span className="ar-helpers">({g.helperCount} helpers)</span>
                        )}
                        {g.isBlockingSleep && <span className="ar-tag red">[blocking]</span>}
                        {!g.isBlockingSleep && g.verdict === "runaway" && (
                          <span className="ar-tag">[runaway]</span>
                        )}
                        {g.isSystemProtected && <span className="ar-tag sys">[system]</span>}
                      </div>
                      {g.mostlyIn && <div className="ar-proof">└ mostly in {g.mostlyIn}</div>}
                    </div>

                    <div className="ar-cols">
                      <span
                        className={`ar-m ${
                          metric === "power"
                            ? g.energyImpact >= 35
                              ? "hot"
                              : "hi"
                            : ""
                        }`}
                      >
                        EI {g.energyImpact.toFixed(1)}
                      </span>
                      <span
                        className={`ar-m ${
                          metric === "cpu" ? (g.cpuPct >= 30 ? "hot" : "hi") : ""
                        }`}
                      >
                        {g.cpuPct.toFixed(1)}%
                      </span>
                      <span className={`ar-m ${metric === "mem" ? "hi" : ""}`}>
                        {formatBytes(g.footprintBytes)}
                      </span>
                      <div className="ar-acts" onClick={(e) => e.stopPropagation()}>
                        {!g.isSystemProtected ? (
                          <>
                            <button
                              className="btn"
                              onClick={() =>
                                handleQuit(
                                  g.primaryPid,
                                  g.primaryStartTimeSecs,
                                  g.name,
                                  g.isSystemProtected,
                                )
                              }
                            >
                              Quit
                            </button>
                            {g.children.length > 0 && (
                              <button
                                className="btn"
                                onClick={() => setExpandedAppId(isExp ? null : g.id)}
                                title="Expand individual helper processes for Force Kill"
                              >
                                {isExp ? "Hide" : "PIDs"}
                              </button>
                            )}
                          </>
                        ) : (
                          <span style={{ color: "#4B5366", fontSize: "10px" }}>protected</span>
                        )}
                      </div>
                    </div>
                  </div>

                  {/* Expanded Named Helper Processes — Force Kill lives strictly on the named PID row! */}
                  <div className="sub-accord">
                    <div className="sub-accord-in">
                      {isExp && g.children.length > 0 && (
                        <div className="sub-list">
                          {g.children.map((ch) => {
                            const chArmed = confirmKillPid === ch.pid;
                            return (
                              <div key={ch.pid} className="sub-item">
                                <span className="sub-item-left">
                                  {ch.name}{" "}
                                  <span style={{ color: "#525A6A" }}>pid {ch.pid}</span>
                                </span>
                                <span className="sub-item-right">
                                  <span>{formatBytes(ch.footprintBytes)}</span>
                                  <span>{ch.cpuPct.toFixed(1)}%</span>
                                  <span>{ch.wakeupsPerSec.toFixed(0)} wk/s</span>
                                  {!ch.isSystemProtected && (
                                    <button
                                      className={`btn ${chArmed ? "armed" : "red"}`}
                                      onClick={() =>
                                        handleKill(
                                          ch.pid,
                                          ch.startTimeSecs,
                                          ch.name,
                                          ch.isSystemProtected,
                                        )
                                      }
                                      title="Click twice to send SIGKILL to this exact PID"
                                    >
                                      {chArmed ? "Confirm Kill" : "Kill"}
                                    </button>
                                  )}
                                </span>
                              </div>
                            );
                          })}
                        </div>
                      )}
                    </div>
                  </div>
                </div>
              );
            })}
          </div>
        ) : (
          /* VIEW 2: PLAIN-ENGLISH SLEEP & BATTERY HEALTH REPORT */
          <div key="sleep" className="view-pane">
            {/* Explicit Period-Scoped Plain-English Verdict */}
            <div
              className={`verdict-card sans ${
                sleepVerdict.tone === "warn"
                  ? "warn"
                  : sleepVerdict.tone === "neutral"
                    ? "neutral"
                    : ""
              }`}
            >
              <div className="vc-title" style={{ justifyContent: "space-between" }}>
                <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                  <span>{sleepVerdict.title}</span>
                  <span className="vc-pill">{sleepVerdict.badge}</span>
                </div>
                <button
                  className="btn mono"
                  onClick={() => void refresh(true, true)}
                  title="Re-scan macOS pmset power log"
                >
                  Refresh
                </button>
              </div>
              <div className="vc-desc">{sleepVerdict.desc}</div>
            </div>

            {/* Literal 24h Battery Curve + Asleep Bands + Silent Wake Spikes */}
            <SleepChart
              timeline={sleep.timeline}
              currentChargePct={batt?.chargePct ?? unplug.currentChargePct}
            />

            {/* Period-Consistent "Where Your Battery Went" Split */}
            <div className="split-strip">
              <div className="mini-stat">
                <div className="ms-top">
                  <span className="ms-lbl sans">While using your Mac</span>
                  <span className="ms-val" style={{ color: "#38BDF8" }}>
                    {unplug.dataStatus === "ok" || unplug.dataStatus === "no_battery_sleep"
                      ? `-${unplug.activeDrainPct}%`
                      : "—"}
                  </span>
                </div>
                <div className="ms-note">
                  {unplug.startedAt
                    ? `${formatDuration(unplug.activeSecs)} active · since ${unplug.startedAt}`
                    : unplug.dataStatus === "on_ac"
                      ? "currently on AC power"
                      : "waiting for next AC unplug boundary"}
                </div>
              </div>
              <div className={`mini-stat ${unplug.asleepDrainPct >= 5 ? "warn" : ""}`}>
                <div className="ms-top">
                  <span className="ms-lbl sans">While asleep (same period)</span>
                  <span
                    className="ms-val"
                    style={{
                      color:
                        unplug.asleepDrainPct >= 5
                          ? "#FB923C"
                          : unplug.sleepSessionCount > 0
                            ? "#4ADE80"
                            : "#7C8596",
                    }}
                  >
                    {unplug.sleepSessionCount > 0 ? `-${unplug.asleepDrainPct}%` : "—"}
                  </span>
                </div>
                <div className="ms-note">
                  {unplug.sleepSessionCount > 0
                    ? `${formatDuration(unplug.sleepSecs)} asleep · ${unplug.darkWakeCount} silent wakes`
                    : "no battery sleep in current unplug period"}
                </div>
              </div>
            </div>

            {/* Two-Column Plain-English Diagnosis + Actionable Zero-Root Fixes */}
            <div className="sleep-two">
              <div>
                <div className="sec-bar">
                  <span>
                    {sleep.sinceUnplugWakeReasons.length > 0
                      ? "why it woke up (since unplug)"
                      : "why it woke up (recent 7d on battery)"}
                  </span>
                  <span>wakes</span>
                </div>
                {scopedReasons.length === 0 ? (
                  <div
                    className="sans"
                    style={{ color: "#6E7687", padding: "10px 0", fontSize: 11.5 }}
                  >
                    No repeated battery silent wakes found in the power log.
                  </div>
                ) : (
                  scopedReasons.slice(0, 4).map((agg) => (
                    <div key={agg.decoded.token} className="cause-row sans">
                      <div className="cr-top">
                        <span className="cr-name">{agg.decoded.label}</span>
                        <span className="cr-cnt mono">{agg.count} wakes</span>
                      </div>
                      <div className="cr-sub">{agg.decoded.explanation}</div>
                    </div>
                  ))
                )}

                {sleep.sessions.length > 0 && (
                  <div style={{ marginTop: 10 }}>
                    <div className="sec-bar">
                      <span>battery sleep sessions (last 7d)</span>
                      <span>drain</span>
                    </div>
                    {sleep.sessions.slice(0, 5).map((s) => {
                      const isExp = expandedSessionId === s.id;
                      return (
                        <div
                          key={s.id}
                          className={`app-wrap ${isExp ? "exp" : ""}`}
                          onClick={() => setExpandedSessionId(isExp ? null : s.id)}
                        >
                          <div className="app-row" style={{ padding: "5px 2px" }}>
                            <div className="ar-left">
                              <div className="ar-top">
                                <span className="chev">▸</span>
                                <span className="ar-name" style={{ fontSize: 11 }}>
                                  {s.startTime} → {s.endTime}
                                </span>
                                <span className="ar-helpers">
                                  ({formatDuration(s.durationSecs)})
                                </span>
                              </div>
                            </div>
                            <span
                              style={{
                                fontSize: 11,
                                fontWeight: 600,
                                color:
                                  s.severity === "critical"
                                    ? "#FB7185"
                                    : s.severity === "warning"
                                      ? "#FB923C"
                                      : "#4ADE80",
                              }}
                            >
                              -{s.drainPct}% · {s.darkWakeCount}w
                            </span>
                          </div>
                          <div className="sub-accord">
                            <div className="sub-accord-in">
                              {isExp && (
                                <div className="sub-list">
                                  {s.wakeEvents.slice(0, 5).map((ev, idx) => (
                                    <div key={`${s.id}-${idx}`} className="sub-item">
                                      <span className="sub-item-left">
                                        {ev.timestamp} · {ev.decoded.label}{" "}
                                        <span style={{ color: "#525A6A" }}>
                                          ({ev.decoded.token})
                                        </span>
                                      </span>
                                      <span className="sub-item-right">
                                        {ev.chargePct !== null ? `${ev.chargePct}%` : ""}
                                      </span>
                                    </div>
                                  ))}
                                </div>
                              )}
                            </div>
                          </div>
                        </div>
                      );
                    })}
                  </div>
                )}
              </div>

              <div>
                <div className="sec-bar">
                  <span>how to stop overnight drain</span>
                  <span>read-only</span>
                </div>
                {auditFixes.length === 0 ? (
                  <div className="fix-box sans">
                    <div className="fb-title" style={{ color: "#4ADE80" }}>
                      Sleep settings look clean
                    </div>
                    <div className="fb-sub">
                      No risky pmset battery settings (like disablesleep or Power Nap on battery)
                      were detected.
                    </div>
                  </div>
                ) : (
                  auditFixes.map((item) => {
                    const isCopied = copiedId === item.id;
                    return (
                      <div key={item.id} className="fix-box sans">
                        <div className="fb-title">
                          <span>{item.title}</span>
                          <span className="mono" style={{ fontSize: 10, color: "#677082" }}>
                            {item.key}={item.currentValue}
                          </span>
                        </div>
                        <div className="fb-sub">{item.explanation}</div>
                        {item.fixCommand && (
                          <div className="fb-cmd mono">
                            <span style={{ overflow: "hidden", textOverflow: "ellipsis" }}>
                              {item.fixCommand}
                            </span>
                            <button
                              className={`btn ${isCopied ? "ok" : ""}`}
                              onClick={() => handleCopyFix(item.id, item.fixCommand!)}
                            >
                              {isCopied ? "Copied ✓" : "Copy"}
                            </button>
                          </div>
                        )}
                      </div>
                    );
                  })
                )}
              </div>
            </div>

            <div className="health-foot">
              <span>
                {batt ? (
                  <>
                    Battery Health:{" "}
                    <strong style={{ color: batt.healthPct >= 80 ? "#4ADE80" : "#FBBF24" }}>
                      {Math.round(batt.healthPct)}% ({batt.healthPct >= 80 ? "Good" : "Service"})
                    </strong>{" "}
                    · {batt.cycleCount} / {batt.designCycleCount} cycles ·{" "}
                    {batt.temperatureC.toFixed(1)}°C
                  </>
                ) : (
                  "Desktop Mac (AC Power)"
                )}
              </span>
              <span>
                {power.adapter?.connected && power.adapter.watts
                  ? `${power.adapter.watts}W USB-C PD connected`
                  : "Zero-root · reads macOS pmset log"}
              </span>
            </div>
          </div>
        )}
      </main>

      {/* Clean Contextual Footer */}
      <footer className="footer">
        <div className="footer-hints">
          {view === "live" ? (
            <>
              <span className="hint" onClick={() => setMetric(metric === "power" ? "cpu" : metric === "cpu" ? "mem" : "power")}>
                <span className="fk">p/u/m</span>metric
              </span>
              <span className="hint">
                <span className="fk">↑↓</span>select
              </span>
              <span className="hint">
                <span className="fk">→</span>pids
              </span>
              <span className="hint">
                <span className="fk">q</span>quit
              </span>
              <span
                className="hint"
                onClick={() => {
                  setFilterOpen(true);
                  setTimeout(() => filterInputRef.current?.focus(), 0);
                }}
              >
                <span className="fk">/</span>filter
              </span>
            </>
          ) : (
            <>
              <span className="hint" onClick={() => setView("live")}>
                <span className="fk">1</span>live
              </span>
              {auditFixes.length > 0 && (
                <span className="hint">
                  <span className="fk">y</span>copy fix
                </span>
              )}
              <span className="hint" onClick={handleToggleAwake}>
                <span className="fk">c</span>awake
              </span>
            </>
          )}
          <span className="hint" onClick={() => void hideDashboard()}>
            <span className="fk">Esc</span>hide
          </span>
        </div>
        {toast ? (
          <span className={`toast ${toast.err ? "err" : ""}`}>{toast.text}</span>
        ) : (
          <span>
            {snap.chipName} · {snap.macosVersion}
          </span>
        )}
      </footer>
    </div>
  );
}
