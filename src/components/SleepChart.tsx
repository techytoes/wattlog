import { useState } from "react";
import type { TimelineBucket } from "../types";

interface SleepChartProps {
  timeline: TimelineBucket[];
  unpluggedAt: string | null;
  currentChargePct: number | null;
}

export function SleepChart({ timeline, unpluggedAt, currentChargePct }: SleepChartProps) {
  const [hoverIdx, setHoverIdx] = useState<number | null>(null);

  if (timeline.length === 0) {
    return (
      <div className="graph-well">
        <div className="gw-head">
          <span>BATTERY OVER LAST 24 HOURS</span>
          <span>waiting for pmset sleep log data</span>
        </div>
        <div style={{ padding: "18px 0", textAlign: "center", color: "#5C6475" }}>
          No recent sleep/wake history yet.
        </div>
      </div>
    );
  }

  const w = 650;
  const chartBottom = 56;
  const chartTop = 8;
  const step = w / Math.max(1, timeline.length - 1);

  // Fill missing charge % smoothly from neighbors
  let lastKnown = currentChargePct ?? 80;
  const charges = timeline.map((b) => {
    if (b.chargePct !== null) {
      lastKnown = b.chargePct;
      return b.chargePct;
    }
    return lastKnown;
  });

  const yOf = (pct: number) =>
    chartBottom - (Math.max(0, Math.min(100, pct)) / 100) * (chartBottom - chartTop);

  const linePath = charges
    .map((pct, i) => `${i === 0 ? "M" : "L"}${(i * step).toFixed(1)} ${yOf(pct).toFixed(1)}`)
    .join(" ");
  const areaPath = `${linePath} L${w} ${chartBottom} L0 ${chartBottom} Z`;

  const hovered = hoverIdx !== null ? timeline[hoverIdx] : null;
  const firstLabel = unpluggedAt ?? timeline[0]?.label ?? "24h ago";
  const midLabel = timeline[Math.floor(timeline.length / 2)]?.label ?? "";

  return (
    <div className="graph-well">
      <div className="gw-head">
        <span>
          {hovered ? (
            <>
              <strong style={{ color: "#FFFFFF" }}>{hovered.label}</strong>
              {hovered.chargePct !== null ? ` · ${hovered.chargePct}%` : ""} ·{" "}
              <span
                style={{
                  color:
                    hovered.state === "darkwake"
                      ? "#FB923C"
                      : hovered.state === "sleep"
                        ? "#4ADE80"
                        : hovered.state === "charging"
                          ? "#A855F7"
                          : "#38BDF8",
                }}
              >
                {hovered.detail}
              </span>
            </>
          ) : (
            "BATTERY OVER LAST 24 HOURS · HOVER TO INSPECT"
          )}
        </span>
        <span style={{ display: "inline-flex", gap: 10 }}>
          <span>
            <span style={{ color: "#38BDF8" }}>━</span> battery %
          </span>
          <span>
            <span style={{ color: "#22C55E" }}>■</span> lid closed
          </span>
          <span>
            <span style={{ color: "#F97316" }}>│</span> silent wakes
          </span>
        </span>
      </div>

      <svg
        viewBox={`0 0 ${w} 74`}
        className="sleep-svg"
        onMouseMove={(e) => {
          const rect = e.currentTarget.getBoundingClientRect();
          const frac = Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width));
          setHoverIdx(Math.round(frac * (timeline.length - 1)));
        }}
        onMouseLeave={() => setHoverIdx(null)}
      >
        <defs>
          <linearGradient id="wattlogBattArea" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor="#38BDF8" stopOpacity="0.24" />
            <stop offset="100%" stopColor="#38BDF8" stopOpacity="0.0" />
          </linearGradient>
        </defs>

        {/* Horizontal 50% and 0% subtle grid */}
        <line
          x1="0"
          y1={yOf(50)}
          x2={w}
          y2={yOf(50)}
          stroke="rgba(255,255,255,0.05)"
          strokeDasharray="2 4"
        />
        <line x1="0" y1={chartBottom} x2={w} y2={chartBottom} stroke="rgba(255,255,255,0.08)" />

        {/* Shaded state bands + DarkWake spikes */}
        {timeline.map((b, i) => {
          const x = i * step;
          const bw = Math.max(step + 0.5, 4);
          if (b.state === "sleep") {
            return (
              <rect
                key={`slp-${i}`}
                x={x - bw / 2}
                y={chartTop}
                width={bw}
                height={chartBottom - chartTop}
                fill="rgba(34, 197, 94, 0.10)"
              />
            );
          }
          if (b.state === "charging") {
            return (
              <rect
                key={`chg-${i}`}
                x={x - bw / 2}
                y={chartTop}
                width={bw}
                height={chartBottom - chartTop}
                fill="rgba(168, 85, 247, 0.09)"
              />
            );
          }
          if (b.state === "darkwake") {
            return (
              <g key={`dw-${i}`}>
                <rect
                  x={x - bw / 2}
                  y={chartTop}
                  width={bw}
                  height={chartBottom - chartTop}
                  fill="rgba(34, 197, 94, 0.08)"
                />
                <line
                  x1={x}
                  y1={yOf(charges[i])}
                  x2={x}
                  y2={chartBottom}
                  stroke="#F97316"
                  strokeWidth="2"
                  opacity="0.88"
                />
              </g>
            );
          }
          if (b.state === "blocked") {
            return (
              <rect
                key={`blk-${i}`}
                x={x - bw / 2}
                y={chartTop}
                width={bw}
                height={chartBottom - chartTop}
                fill="rgba(239, 68, 68, 0.14)"
              />
            );
          }
          return null;
        })}

        {/* Battery % area & crisp curve */}
        <path d={areaPath} fill="url(#wattlogBattArea)" />
        <path
          d={linePath}
          fill="none"
          stroke="#38BDF8"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
        />

        {/* Hover scrub vertical cursor */}
        {hoverIdx !== null && hoverIdx < timeline.length && (
          <>
            <line
              x1={hoverIdx * step}
              y1={chartTop}
              x2={hoverIdx * step}
              y2={chartBottom}
              stroke="rgba(255,255,255,0.28)"
              strokeWidth="1"
            />
            <circle cx={hoverIdx * step} cy={yOf(charges[hoverIdx])} r="3.4" fill="#FFFFFF" />
          </>
        )}

        {/* Axis labels */}
        <text x="0" y="70" fill="#636C80" fontSize="9.5" fontFamily="SF Mono, monospace">
          {firstLabel}
        </text>
        <text
          x={w / 2}
          y="70"
          textAnchor="middle"
          fill="#636C80"
          fontSize="9.5"
          fontFamily="SF Mono, monospace"
        >
          {midLabel}
        </text>
        <text
          x={w}
          y="70"
          textAnchor="end"
          fill="#CBD3E1"
          fontSize="9.5"
          fontFamily="SF Mono, monospace"
        >
          Now{currentChargePct !== null ? ` (${currentChargePct}%)` : ""}
        </text>
      </svg>
    </div>
  );
}
