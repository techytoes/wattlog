export type PowerState = "discharging" | "charging" | "full" | "acOnly";

export interface BatteryInfo {
  chargePct: number;
  healthPct: number;
  rawHealthPct: number;
  cycleCount: number;
  designCycleCount: number;
  voltageMv: number;
  amperageMa: number;
  temperatureC: number;
  timeRemainingMins: number | null;
  externalConnected: boolean;
  isCharging: boolean;
  fullyCharged: boolean;
  nominalCapacityMah: number;
  rawMaxCapacityMah: number;
  designCapacityMah: number;
  currentCapacityMah: number;
}

export interface AdapterInfo {
  connected: boolean;
  watts: number | null;
  voltageMv: number | null;
  currentMa: number | null;
  name: string | null;
  description: string | null;
  isWireless: boolean;
}

export interface ComponentPower {
  cpuWatts: number;
  gpuWatts: number;
  aneWatts: number;
  dramWatts: number;
  sysWatts: number;
  displayOtherWatts: number;
  cpuTempC: number | null;
  gpuTempC: number | null;
  cpuActivePct: number;
  gpuActivePct: number;
  fanRpm: number | null;
  available: boolean;
}

export interface PowerSnapshot {
  systemWatts: number;
  batteryWatts: number;
  sourceTier: string;
  state: PowerState;
  battery: BatteryInfo | null;
  adapter: AdapterInfo | null;
  components: ComponentPower | null;
}

export interface DecodedWakeReason {
  token: string;
  label: string;
  category: string;
  explanation: string;
  tip: string;
  isInsomniaSuspect: boolean;
}

export interface WakeEvent {
  timestamp: string;
  kind: string;
  durationSecs: number;
  chargePct: number | null;
  powerSource: string;
  rawReason: string;
  decoded: DecodedWakeReason;
}

export interface SleepSession {
  id: string;
  startTime: string;
  endTime: string;
  durationSecs: number;
  startChargePct: number | null;
  endChargePct: number | null;
  drainPct: number;
  powerSource: string;
  sleepReason: string;
  darkWakeCount: number;
  darkWakeTotalSecs: number;
  worstWakeReason: DecodedWakeReason | null;
  wakeEvents: WakeEvent[];
  severity: "ok" | "warning" | "critical";
}

export interface WakeReasonAggregate {
  decoded: DecodedWakeReason;
  count: number;
  totalSecs: number;
}

export interface TimelineBucket {
  label: string;
  state: "active" | "darkwake" | "sleep" | "blocked" | "charging" | "empty";
  watts: number;
  chargePct: number | null;
  durationSecs: number;
  detail: string;
}

export interface SinceUnplugSummary {
  startedAt: string | null;
  elapsedSecs: number;
  startChargePct: number | null;
  currentChargePct: number | null;
  totalDropPct: number;
  activeSecs: number;
  sleepSecs: number;
  darkWakeSecs: number;
  darkWakeCount: number;
  drainPctPerHour: number;
  onBatteryNow: boolean;
}

export interface SleepReport {
  sinceUnplug: SinceUnplugSummary;
  sessions: SleepSession[];
  topWakeReasons: WakeReasonAggregate[];
  timeline: TimelineBucket[];
  rawLogAvailable: boolean;
}

export interface SleepBlocker {
  pid: number;
  processName: string;
  appName: string;
  bundlePath: string | null;
  assertionType: string;
  assertionName: string;
  details: string | null;
  durationSecs: number;
  severity: "critical" | "warning" | "system";
  isSystemProtected: boolean;
  isSelfWattlog: boolean;
  blocksDisplay: boolean;
  blocksSystem: boolean;
}

export interface KeepAwakeConfig {
  enabled: boolean;
  mode: "display" | "system";
  durationMins: number | null;
  autoOffBelowBatteryPct: number | null;
  autoOffOnUnplug: boolean;
  watchAppName: string | null;
}

export interface KeepAwakeState {
  active: boolean;
  mode: string;
  remainingSecs: number | null;
  startedAtMs: number | null;
  autoOffBelowBatteryPct: number | null;
  autoOffOnUnplug: boolean;
  watchAppName: string | null;
  watchAppRunning: boolean;
  lastAutoReleaseReason: string | null;
  assertionId: number | null;
}

export interface ProcessRow {
  pid: number;
  ppid: number;
  name: string;
  path: string | null;
  cpuPct: number;
  footprintBytes: number;
  energyImpact: number;
  wakeupsPerSec: number;
  isSystemProtected: boolean;
}

export interface AppProcessGroup {
  id: string;
  name: string;
  primaryPid: number;
  bundlePath: string | null;
  helperCount: number;
  cpuPct: number;
  footprintBytes: number;
  energyImpact: number;
  wakeupsPerSec: number;
  verdict: "runaway" | "blocking" | "elevated" | "system" | "normal";
  isSystemProtected: boolean;
  isBlockingSleep: boolean;
  mostlyIn: string | null;
  children: ProcessRow[];
}

export interface MemoryPressureInfo {
  level: "green" | "yellow" | "red";
  totalBytes: number;
  usedBytes: number;
  wiredBytes: number;
  compressedBytes: number;
  swapUsedBytes: number;
  swapTotalBytes: number;
}

export interface ProcessReport {
  groups: AppProcessGroup[];
  memoryPressure: MemoryPressureInfo;
  totalCpuPct: number;
  totalFootprintBytes: number;
  kernelThermalNote: string | null;
}

export interface PmsetAuditItem {
  id: string;
  profile: string;
  key: string;
  currentValue: string;
  recommendedValue: string;
  severity: "critical" | "warning" | "ok";
  title: string;
  explanation: string;
  fixCommand: string | null;
}

export interface SampleRecord {
  timestampMs: number;
  systemWatts: number;
  batteryPct: number | null;
  isCharging: boolean;
  externalConnected: boolean;
  cpuWatts: number | null;
  gpuWatts: number | null;
  aneWatts: number | null;
  state: string;
  memoryPressure: string;
}

export interface DashboardSnapshot {
  timestampMs: number;
  demoMode: boolean;
  hostName: string;
  chipName: string;
  macosVersion: string;
  power: PowerSnapshot;
  sleep: SleepReport;
  blockers: SleepBlocker[];
  keepAwake: KeepAwakeState;
  processes: ProcessReport;
  audit: PmsetAuditItem[];
  history: SampleRecord[];
}

export interface ActionResult {
  ok: boolean;
  message: string;
}

export type RailCategoryId = "unplug" | "blockers" | "awake" | "apps" | "audit";
