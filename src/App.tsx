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
} from "./components/ReactiveWaveform";
import { SleepChart } from "./components/SleepChart";
import { formatBytes, formatDuration, formatEta } from "./format";
import type { DashboardSnapshot, KeepAwakeConfig } from "./types";

type TopView = "live" | "sleep";

const MAX_HISTORY_PTS = 40;

export function App() {
  const [snap, setSnap] = useState<DashboardSnapshot | null>(null);
  const [view, setView] = useState<TopView>("live");
  const [metric, setMetric] = useState<LiveMetricMode>("power");
  const [awakeDrawerOpen, setAwakeDrawerOpen] = useState(false);
  const [selectedIdx, setSelectedIdx] = useState(0);
  const [expandedAppId, setExpandedAppId] = useState<string | null>(null);
  const [expandedSessionId, setExpandedSessionId] = useState<string | null>(null);
  const [showSystemBlockers, setShowSystemBlockers] = useState(false);
  const [filterOpen, setFilterOpen] = useState(false);
  const [filterText, setFilterText] = useState("");
  const [confirmKillPid, setConfirmKillPid] = useState<number | null>(null);
  const [copiedId, setCopiedId] = useState<string | null>(null);
  const [toast, setToast] = useState<{ text: string; err?: boolean } | null>(null);

  // Rolling 60s live sample buffers for OLED waveforms
  const [powerSeries, setPowerSeries] = useState<number[]>([]);
  const [cpuSeries, setCpuSeries] = useState<number[]>([]);
  const [memSeries, setMemSeries] = useState<number[]>([]);

  // Gliding pill indicator refs
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
    async (forceSleep = false) => {
      try {
        const next = await fetchSnapshot(forceSleep);
        setSnap(next);

        const w = next.power.systemWatts;
        const cpuVal = next.power.components?.available
          ? next.power.components.cpuActivePct
          : Math.min(100, next.processes.totalCpuPct / 4);
        const memGb = next.processes.memoryPressure.usedBytes / 1_073_741_824;

        setPowerSeries((prev) => {
          if (prev.length === 0) {
            if (next.history.length >= 4) {
              return next.history.slice(-MAX_HISTORY_PTS).map((h) => h.systemWatts);
            }
            // Seed initial calm buffer so the waveform renders smoothly immediately
            return Array.from({ length: 16 }, (_, i) =>
              Math.max(0.5, w + Math.sin(i * 0.5) * 0.25),
            );
          }
          return [...prev.slice(-(MAX_HISTORY_PTS - 1)), w];
        });

        setCpuSeries((prev) => {
          if (prev.length === 0) {
            return Array.from({ length: 16 }, (_, i) =>
              Math.max(1, cpuVal + Math.sin(i * 0.6) * 1.2),
            );
          }
          return [...prev.slice(-(MAX_HISTORY_PTS - 1)), cpuVal];
        });

        setMemSeries((prev) => {
          if (prev.length === 0) {
            return Array.from({ length: 16 }, () => memGb);
          }
          return [...prev.slice(-(MAX_HISTORY_PTS - 1)), memGb];
        });
      } catch (e) {
        showToast(`telemetry error: ${String(e)}`, true);
      }
    },
    [showToast],
  );

  // Live 1.5s polling ONLY while visible
  useEffect(() => {
    refresh(true);
    const id = setInterval(() => {
      if (document.visibilityState === "visible") {
        refresh(false);
      }
    }, 1500);
    return () => clearInterval(id);
  }, [refresh]);

  // Position gliding segmented indicator thumb smoothly
  const syncThumb = useCallback(() => {
    const btn = view === "live" ? liveTabRef.current : sleepTabRef.current;
    const thumb = thumbRef.current;
    if (!btn || !thumb) return;
    thumb.style.width = `${btn.offsetWidth}px`;
    thumb.style.transform = `translateX(${btn.offsetLeft - 2}px)`;
  }, [view]);

  useLayoutEffect(() => {
    syncThumb();
  }, [syncThumb, snap?.demoMode]);

  useEffect(() => {
    window.addEventListener("resize", syncThumb);
    return () => window.removeEventListener("resize", syncThumb);
  }, [syncThumb]);

  const query = filterText.trim().toLowerCase();

  // Auto-sort apps to match active metric tile (Power -> Energy, CPU -> CPU%, Memory -> Footprint)
  const visibleApps = useMemo(() => {
    if (!snap) return [];
    const filtered = snap.processes.groups.filter((g) => {
      if (!query) return true;
      return `${g.name} ${g.mostlyIn ?? ""} ${g.verdict}`.toLowerCase().includes(query);
    });
    return [...filtered].sort((a, b) => {
      if (metric === "cpu") return b.cpuPct - a.cpuPct;
      if (metric === "mem") return b.footprintBytes - a.footprintBytes;
      // Power / Energy Impact (sleep blockers bubble up slightly)
      const scoreA = (a.isBlockingSleep ? 25 : 0) + a.energyImpact;
      const scoreB = (b.isBlockingSleep ? 25 : 0) + b.energyImpact;
      return scoreB - scoreA || b.footprintBytes - a.footprintBytes;
    });
  }, [snap, query, metric]);

  useEffect(() => {
    setSelectedIdx((i) => (visibleApps.length === 0 ? 0 : Math.min(i, visibleApps.length - 1)));
  }, [visibleApps.length, metric]);

  const handleToggleAwake = useCallback(async () => {
    try {
      const ka = await toggleKeepAwake();
      showToast(
        ka.active
          ? `keep-awake ON (${ka.mode}${ka.remainingSecs ? ` · ${formatDuration(ka.remainingSecs)}` : ""})`
          : ka.lastAutoReleaseReason ?? "keep-awake OFF",
      );
      refresh(false);
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
        refresh(false);
      } catch (e) {
        showToast(String(e), true);
      }
    },
    [snap, refresh, showToast],
  );

  const handleQuit = useCallback(
    async (pid: number, name: string, protectedSys: boolean) => {
      if (protectedSys) {
        showToast(`protected macOS system process: ${name}`, true);
        return;
      }
      const res = await quitProcess(pid, name);
      showToast(res.message, !res.ok);
      setTimeout(() => refresh(false), 350);
    },
    [refresh, showToast],
  );

  const handleKill = useCallback(
    async (pid: number, name: string, protectedSys: boolean) => {
      if (protectedSys) {
        showToast(`protected macOS system process: ${name}`, true);
        return;
      }
      if (confirmKillPid !== pid) {
        setConfirmKillPid(pid);
        showToast(`press Kill / x again to SIGKILL ${name} (pid ${pid})`, true);
        return;
      }
      setConfirmKillPid(null);
      const res = await forceKillProcess(pid, true);
      showToast(res.message, !res.ok);
      setTimeout(() => refresh(false), 350);
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

  // Keyboard shortcuts
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (filterOpen) {
        if (e.key === "Escape" || e.key === "Enter") {
          e.preventDefault();
          setFilterOpen(false);
          if (e.key === "Escape") setFilterText("");
        }
        return;
      }
      if (e.metaKey || e.ctrlKey || e.altKey) return;

      if (e.key === "Escape") {
        e.preventDefault();
        if (confirmKillPid !== null) return setConfirmKillPid(null);
        if (awakeDrawerOpen) return setAwakeDrawerOpen(false);
        if (expandedAppId) return setExpandedAppId(null);
        if (filterText) return setFilterText("");
        hideDashboard();
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
      if (e.key === "c") {
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
        if (e.key === "ArrowDown" || e.key === "j") {
          e.preventDefault();
          setConfirmKillPid(null);
          setSelectedIdx((i) => Math.min(Math.max(0, visibleApps.length - 1), i + 1));
        } else if (e.key === "ArrowUp" || e.key === "k") {
          e.preventDefault();
          setConfirmKillPid(null);
          setSelectedIdx((i) => Math.max(0, i - 1));
        } else if (e.key === "ArrowRight" || e.key === "Enter") {
          const app = visibleApps[selectedIdx];
          if (app) {
            e.preventDefault();
            setExpandedAppId((cur) => (cur === app.id ? null : app.id));
          }
        } else if (e.key === "ArrowLeft") {
          setExpandedAppId(null);
        } else if (e.key === "q") {
          const app = visibleApps[selectedIdx];
          if (app) {
            e.preventDefault();
            void handleQuit(app.primaryPid, app.name, app.isSystemProtected);
          }
        } else if (e.key === "x") {
          const app = visibleApps[selectedIdx];
          if (app) {
            e.preventDefault();
            const target = app.children[0];
            void handleKill(
              target ? target.pid : app.primaryPid,
              target ? target.name : app.name,
              app.isSystemProtected,
            );
          }
        }
      } else if (view === "sleep" && e.key === "y" && snap) {
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
    selectedIdx,
    snap,
    handleToggleAwake,
    handleQuit,
    handleKill,
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

  // Compute waveform props based on selected metric tile
  const totalGb = Math.max(1, mp.totalBytes / 1_073_741_824);
  const usedGb = mp.usedBytes / 1_073_741_824;
  const cpuDisplayPct = comp?.available
    ? comp.cpuActivePct
    : Math.min(100, processes.totalCpuPct / 4);
  const gpuDisplayPct = comp?.available ? comp.gpuActivePct : 0;

  const powerPeak = Math.max(power.systemWatts, ...powerSeries);
  const powerAvg =
    powerSeries.length > 0
      ? powerSeries.reduce((a, b) => a + b, 0) / powerSeries.length
      : power.systemWatts;

  const powerRail: RailSegment[] = (() => {
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
    return [{ label: `${power.sourceTier} · ${power.systemWatts.toFixed(1)}W`, pct: 100, color: "#38BDF8" }];
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

  const waveformConfig =
    metric === "power"
      ? {
          points: powerSeries,
          maxVal: 24,
          unit: "W",
          title: "POWER DRAW · LOAD-REACTIVE WAVEFORM (60S)",
          rightMeta: (
            <>
              peak <strong style={{ color: "#FB923C" }}>{powerPeak.toFixed(1)}W</strong> · avg{" "}
              {powerAvg.toFixed(1)}W
            </>
          ),
          rail: powerRail,
        }
      : metric === "cpu"
        ? {
            points: cpuSeries,
            maxVal: 100,
            unit: "%",
            title: "CPU & GPU ACTIVITY · LAST 60 SECONDS",
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

  // Plain-English Sleep Health computation
  const worstSession = sleep.sessions.find((s) => s.severity !== "ok") ?? sleep.sessions[0] ?? null;
  const totalSleepDrain = sleep.sessions.reduce((acc, s) => acc + s.drainPct, 0);
  const asleepDrain = worstSession ? worstSession.drainPct : totalSleepDrain;
  const activeDrain = Math.max(0, sleep.sinceUnplug.totalDropPct - asleepDrain);
  const hasSleepWarning =
    (worstSession && (worstSession.drainPct >= 5 || worstSession.darkWakeCount >= 6)) ||
    auditFixes.length > 0;

  const sleepVerdict = (() => {
    if (!worstSession) {
      return {
        warn: false,
        title: "No Overnight Drain Detected",
        badge: "clean",
        desc: "Your Mac hasn't had a noisy sleep session in the recent macOS power log.",
      };
    }
    if (worstSession.drainPct >= 8 || worstSession.darkWakeCount >= 10) {
      const top = worstSession.worstWakeReason;
      return {
        warn: true,
        title: "Restless Sleep Detected",
        badge: `-${worstSession.drainPct}% while closed`,
        desc: `Your Mac woke silently ${worstSession.darkWakeCount} times during ${formatDuration(worstSession.durationSecs)} of sleep${top ? ` — mostly from ${top.label}` : ""}. Healthy sleep should lose under 3% overnight.`,
      };
    }
    if (worstSession.drainPct >= 3 || worstSession.darkWakeCount >= 3) {
      const top = worstSession.worstWakeReason;
      return {
        warn: true,
        title: "Light Background Wakes During Sleep",
        badge: `${worstSession.darkWakeCount} silent wakes`,
        desc: `Lost ${worstSession.drainPct}% over ${formatDuration(worstSession.durationSecs)}${top ? `, triggered mainly by ${top.label}` : ""}.`,
      };
    }
    return {
      warn: false,
      title: "Healthy Deep Sleep",
      badge: `-${worstSession.drainPct}% overnight`,
      desc: `Your Mac stayed cleanly in deep sleep (${worstSession.darkWakeCount} silent wakes over ${formatDuration(worstSession.durationSecs)}). Nothing urgent to fix.`,
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
            >
              {awakeDrawerOpen ? "▴" : "▾"}
            </button>
          </div>
        </div>
      </header>

      {/* Smooth Unfolding Keep-Awake Inline Drawer */}
      <div className={`drawer-wrap ${awakeDrawerOpen ? "open" : ""}`}>
        <div className="drawer-inner">
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
        </div>
      </div>

      {/* Main Content Stage */}
      <main className="stage">
        {view === "live" ? (
          <div key="live" className="view-pane">
            {/* 3-Metric Interactive Strip: Power · CPU & GPU · Memory ALL visible simultaneously */}
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
                  <MiniSparkline points={powerSeries} maxVal={24} color="#38BDF8" />
                </div>
                <div className="mt-val-row">
                  <span className="mt-val">
                    {power.systemWatts.toFixed(1)}
                    <span className="mt-unit">W</span>
                  </span>
                  <span
                    style={{
                      fontSize: "10px",
                      color:
                        power.systemWatts >= 16
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
                  <MiniSparkline points={cpuSeries} maxVal={100} color="#FBBF24" />
                </div>
                <div className="mt-val-row">
                  <span className="mt-val">
                    {Math.round(cpuDisplayPct)}
                    <span className="mt-unit">%</span>
                  </span>
                  <span style={{ fontSize: "10px", color: "#FBBF24" }}>
                    {comp?.cpuTempC ? `${comp.cpuTempC.toFixed(0)}°C` : `${processes.totalCpuPct.toFixed(0)}% sum`}
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

            {/* Pinned Inline Sleep Blockers Strip (Only when user apps are blocking sleep!) */}
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
                      className={`btn ${confirmKillPid === b.pid ? "armed" : "red"}`}
                      onClick={() => handleQuit(b.pid, b.appName, b.isSystemProtected)}
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

            {/* Optional Expanded Assertions Drawer */}
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
                          onClick={() => handleQuit(b.pid, b.appName, b.isSystemProtected)}
                        >
                          Quit
                        </button>
                      )}
                    </span>
                  </div>
                ))}
              </div>
            )}

            {/* kernel_task Thermal Throttling Callout */}
            {processes.kernelThermalNote && (
              <div className="thermal-strip">└ {processes.kernelThermalNote}</div>
            )}

            {/* Optional Quick Filter Input */}
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

            {visibleApps.map((g, i) => {
              const isSel = selectedIdx === i;
              const isExp = expandedAppId === g.id;
              const topChild = g.children[0];
              const killPid = topChild ? topChild.pid : g.primaryPid;
              const armed = confirmKillPid === killPid;

              return (
                <div
                  key={g.id}
                  className={`app-wrap ${isSel ? "selected" : ""} ${isExp ? "exp" : ""}`}
                >
                  <div
                    className="app-row"
                    onClick={() => {
                      setSelectedIdx(i);
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
                                handleQuit(g.primaryPid, g.name, g.isSystemProtected)
                              }
                            >
                              Quit
                            </button>
                            <button
                              className={`btn ${armed ? "armed" : "red"}`}
                              onClick={() =>
                                handleKill(
                                  killPid,
                                  topChild ? topChild.name : g.name,
                                  g.isSystemProtected,
                                )
                              }
                              title="Click twice (or press x twice) to SIGKILL"
                            >
                              {armed ? "Confirm" : "Kill"}
                            </button>
                          </>
                        ) : (
                          <span style={{ color: "#4B5366", fontSize: "10px" }}>protected</span>
                        )}
                      </div>
                    </div>
                  </div>

                  {/* Smooth Accordion Helper Sub-Processes */}
                  <div className="sub-accord">
                    <div className="sub-accord-in">
                      {g.children.length > 0 && (
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
                                        handleKill(ch.pid, ch.name, ch.isSystemProtected)
                                      }
                                    >
                                      {chArmed ? "Confirm" : "Kill"}
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
            {/* 1-Sentence Plain-English Sleep Verdict */}
            <div className={`verdict-card sans ${sleepVerdict.warn ? "warn" : ""}`}>
              <div className="vc-title">
                <span>{sleepVerdict.title}</span>
                <span className="vc-pill">{sleepVerdict.badge}</span>
              </div>
              <div className="vc-desc">{sleepVerdict.desc}</div>
            </div>

            {/* 24h Glowing Battery Curve + Shaded Sleep Bands + Silent Wake Spikes */}
            <SleepChart
              timeline={sleep.timeline}
              unpluggedAt={sleep.sinceUnplug.startedAt}
              currentChargePct={batt?.chargePct ?? sleep.sinceUnplug.currentChargePct}
            />

            {/* Simple "Where Your Battery Went" Two-Card Split */}
            <div className="split-strip">
              <div className="mini-stat">
                <div className="ms-top">
                  <span className="ms-lbl sans">While using your Mac</span>
                  <span className="ms-val" style={{ color: "#38BDF8" }}>
                    -{activeDrain}%
                  </span>
                </div>
                <div className="ms-note">
                  {formatDuration(sleep.sinceUnplug.activeSecs)} active
                  {sleep.sinceUnplug.startedAt
                    ? ` · unplugged ${sleep.sinceUnplug.startedAt}`
                    : ""}
                </div>
              </div>
              <div className={`mini-stat ${asleepDrain >= 5 ? "warn" : ""}`}>
                <div className="ms-top">
                  <span className="ms-lbl sans">While asleep (lid closed)</span>
                  <span
                    className="ms-val"
                    style={{ color: asleepDrain >= 5 ? "#FB923C" : "#4ADE80" }}
                  >
                    -{asleepDrain}%
                  </span>
                </div>
                <div className="ms-note">
                  {formatDuration(sleep.sinceUnplug.sleepSecs)} asleep ·{" "}
                  {sleep.sinceUnplug.darkWakeCount} silent wakes
                </div>
              </div>
            </div>

            {/* Two-Column Diagnosis (Plain English) + Actionable Zero-Root Fixes */}
            <div className="sleep-two">
              <div>
                <div className="sec-bar">
                  <span>why it woke up (plain english)</span>
                  <span>wakes</span>
                </div>
                {sleep.topWakeReasons.length === 0 ? (
                  <div className="sans" style={{ color: "#6E7687", padding: "10px 0", fontSize: 11.5 }}>
                    No repeated silent wakes found in recent sleep sessions.
                  </div>
                ) : (
                  sleep.topWakeReasons.slice(0, 4).map((agg) => (
                    <div key={agg.decoded.token} className="cause-row sans">
                      <div className="cr-top">
                        <span className="cr-name">{agg.decoded.label}</span>
                        <span className="cr-cnt mono">{agg.count} wakes</span>
                      </div>
                      <div className="cr-sub">{agg.decoded.explanation}</div>
                    </div>
                  ))
                )}

                {/* Expandable Recent Sleep Sessions */}
                {sleep.sessions.length > 0 && (
                  <div style={{ marginTop: 10 }}>
                    <div className="sec-bar">
                      <span>recent sleep sessions</span>
                      <span>drain</span>
                    </div>
                    {sleep.sessions.slice(0, 4).map((s) => {
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

            {/* Quiet Battery Longevity & Charger Footer */}
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

      {/* Bottom Keybind & Toast Footer */}
      <footer className="footer">
        <div className="footer-hints">
          <span className="hint" onClick={() => { setView("live"); setMetric("power"); }}>
            <span className="fk">p</span>power
          </span>
          <span className="hint" onClick={() => { setView("live"); setMetric("cpu"); }}>
            <span className="fk">u</span>cpu
          </span>
          <span className="hint" onClick={() => { setView("live"); setMetric("mem"); }}>
            <span className="fk">m</span>memory
          </span>
          <span className="hint" onClick={handleToggleAwake}>
            <span className="fk">c</span>awake
          </span>
          <span className="hint" onClick={() => setView(view === "live" ? "sleep" : "live")}>
            <span className="fk">1/2</span>views
          </span>
          <span
            className="hint"
            onClick={() => {
              setView("live");
              setFilterOpen(true);
              setTimeout(() => filterInputRef.current?.focus(), 0);
            }}
          >
            <span className="fk">/</span>filter
          </span>
          <span className="hint" onClick={() => hideDashboard()}>
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
