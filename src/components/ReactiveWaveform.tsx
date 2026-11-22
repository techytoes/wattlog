import { useEffect, useMemo, useRef, useState } from "react";

export type LiveMetricMode = "power" | "cpu" | "mem";

export interface TimedPoint {
  ts: number;
  val: number;
}

export interface RailSegment {
  label: string;
  pct: number;
  color: string;
}

interface ReactiveWaveformProps {
  mode: LiveMetricMode;
  points: TimedPoint[];
  maxVal: number;
  unit: string;
  title: string;
  rightMeta: React.ReactNode;
  rail: RailSegment[];
}

function colorForValue(mode: LiveMetricMode, v: number, maxVal: number): string {
  const ratio = maxVal > 0 ? v / maxVal : 0;
  if (mode === "mem") {
    if (ratio >= 0.85) return "#F97316";
    if (ratio >= 0.72) return "#FBBF24";
    return "#22C55E";
  }
  if (mode === "power") {
    if (v >= 16) return "#F97316";
    if (v >= 10) return "#FBBF24";
    return "#38BDF8";
  }
  if (v >= 65) return "#F97316";
  if (v >= 35) return "#FBBF24";
  return "#38BDF8";
}

const CANVAS_H = 76;

/**
 * Zero-waste OLED load-reactive waveform:
 * - Plots real timestamped observations (grows honestly from open, zero fake sine wave).
 * - Redraws ONLY on new sample, resize, mode switch, or hover (no 60fps canvas burn!).
 * - Endpoint pulse is handled by a tiny GPU-composited CSS dot.
 */
export function ReactiveWaveform({
  mode,
  points,
  maxVal,
  unit,
  title,
  rightMeta,
  rail,
}: ReactiveWaveformProps) {
  const wrapRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [hoverIdx, setHoverIdx] = useState<number | null>(null);
  const [width, setWidth] = useState(660);

  useEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    const sync = () => {
      const w = Math.floor(el.getBoundingClientRect().width) - 24;
      if (w > 120) setWidth(w);
    };
    sync();
    const ro = new ResizeObserver(sync);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const ceiling = useMemo(() => {
    if (points.length === 0) return Math.max(maxVal, 1);
    return Math.max(maxVal, ...points.map((p) => p.val * 1.15), 1);
  }, [points, maxVal]);

  const yOf = (v: number) => CANVAS_H - 8 - (Math.max(0, v) / ceiling) * (CANVAS_H - 16);

  // Redraw canvas strictly when data, dimensions, mode, or hover changes.
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const dpr = window.devicePixelRatio || 1;
    if (canvas.width !== Math.floor(width * dpr) || canvas.height !== Math.floor(CANVAS_H * dpr)) {
      canvas.width = Math.floor(width * dpr);
      canvas.height = Math.floor(CANVAS_H * dpr);
      canvas.style.width = `${width}px`;
      canvas.style.height = `${CANVAS_H}px`;
    }

    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, width, CANVAS_H);

    // Subtle threshold guides
    ctx.setLineDash([2, 4]);
    ctx.lineWidth = 1;
    if (mode === "power" && ceiling >= 16) {
      ctx.strokeStyle = "rgba(249, 115, 22, 0.16)";
      ctx.beginPath();
      ctx.moveTo(0, yOf(16));
      ctx.lineTo(width, yOf(16));
      ctx.stroke();
    }
    ctx.strokeStyle = "rgba(255, 255, 255, 0.055)";
    [0.32, 0.66].forEach((f) => {
      const y = Math.round(CANVAS_H * f) + 0.5;
      ctx.beginPath();
      ctx.moveTo(0, y);
      ctx.lineTo(width, y);
      ctx.stroke();
    });
    ctx.setLineDash([]);

    if (points.length === 0) return;

    const coords =
      points.length === 1
        ? [
            { x: 0, y: yOf(points[0].val) },
            { x: width - 2, y: yOf(points[0].val) },
          ]
        : points.map((p, i) => ({
            x: (i / (points.length - 1)) * (width - 2),
            y: yOf(p.val),
          }));

    const sGrad = ctx.createLinearGradient(0, 6, 0, CANVAS_H - 6);
    const aGrad = ctx.createLinearGradient(0, 6, 0, CANVAS_H);
    if (mode === "mem") {
      sGrad.addColorStop(0.0, "#F97316");
      sGrad.addColorStop(0.32, "#FBBF24");
      sGrad.addColorStop(0.68, "#22C55E");
      sGrad.addColorStop(1.0, "#10B981");
      aGrad.addColorStop(0.0, "rgba(34, 197, 94, 0.28)");
      aGrad.addColorStop(1.0, "rgba(34, 197, 94, 0.0)");
    } else if (mode === "cpu") {
      sGrad.addColorStop(0.0, "#FB7185");
      sGrad.addColorStop(0.32, "#F97316");
      sGrad.addColorStop(0.62, "#FBBF24");
      sGrad.addColorStop(1.0, "#38BDF8");
      aGrad.addColorStop(0.0, "rgba(251, 191, 36, 0.30)");
      aGrad.addColorStop(0.55, "rgba(56, 189, 248, 0.12)");
      aGrad.addColorStop(1.0, "rgba(56, 189, 248, 0.0)");
    } else {
      sGrad.addColorStop(0.0, "#FB7185");
      sGrad.addColorStop(0.28, "#F97316");
      sGrad.addColorStop(0.58, "#FBBF24");
      sGrad.addColorStop(0.84, "#38BDF8");
      sGrad.addColorStop(1.0, "#22D3EE");
      aGrad.addColorStop(0.0, "rgba(249, 115, 22, 0.32)");
      aGrad.addColorStop(0.45, "rgba(251, 191, 36, 0.15)");
      aGrad.addColorStop(0.78, "rgba(56, 189, 248, 0.09)");
      aGrad.addColorStop(1.0, "rgba(56, 189, 248, 0.0)");
    }

    const trace = () => {
      ctx.beginPath();
      coords.forEach((pt, i) => {
        if (i === 0) ctx.moveTo(pt.x, pt.y);
        else {
          const prev = coords[i - 1];
          const cx = (prev.x + pt.x) / 2;
          ctx.bezierCurveTo(cx, prev.y, cx, pt.y, pt.x, pt.y);
        }
      });
    };

    trace();
    ctx.lineTo(width - 2, CANVAS_H);
    ctx.lineTo(0, CANVAS_H);
    ctx.closePath();
    ctx.fillStyle = aGrad;
    ctx.fill();

    trace();
    ctx.strokeStyle = sGrad;
    ctx.lineWidth = 5;
    ctx.globalAlpha = 0.2;
    ctx.stroke();

    trace();
    ctx.globalAlpha = 1;
    ctx.lineWidth = 2.1;
    ctx.stroke();

    if (hoverIdx !== null && hoverIdx >= 0 && hoverIdx < points.length && points.length >= 2) {
      const hx = (hoverIdx / (points.length - 1)) * (width - 2);
      const hy = yOf(points[hoverIdx].val);
      ctx.strokeStyle = "rgba(255, 255, 255, 0.24)";
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(hx, 4);
      ctx.lineTo(hx, CANVAS_H - 4);
      ctx.stroke();

      ctx.beginPath();
      ctx.arc(hx, hy, 4.2, 0, Math.PI * 2);
      ctx.fillStyle = colorForValue(mode, points[hoverIdx].val, ceiling);
      ctx.fill();

      ctx.beginPath();
      ctx.arc(hx, hy, 2, 0, Math.PI * 2);
      ctx.fillStyle = "#FFFFFF";
      ctx.fill();
    }
  }, [width, mode, points, ceiling, hoverIdx]);

  const lastPt = points.length > 0 ? points[points.length - 1] : null;
  const beaconY = lastPt ? yOf(lastPt.val) : CANVAS_H / 2;
  const beaconColor = lastPt ? colorForValue(mode, lastPt.val, ceiling) : "#38BDF8";

  const hoveredPt =
    hoverIdx !== null && hoverIdx >= 0 && hoverIdx < points.length ? points[hoverIdx] : null;
  const hoveredAgoSecs =
    hoveredPt && lastPt ? Math.max(0, Math.round((lastPt.ts - hoveredPt.ts) / 1000)) : 0;

  return (
    <div className="graph-well" ref={wrapRef}>
      <div className="gw-head">
        <span>
          {hoveredPt !== null ? (
            <>
              <strong style={{ color: colorForValue(mode, hoveredPt.val, ceiling) }}>
                {hoveredPt.val.toFixed(1)}
                {unit}
              </strong>{" "}
              · {hoveredAgoSecs <= 0 ? "live" : `${hoveredAgoSecs}s ago`}
            </>
          ) : (
            title
          )}
        </span>
        <span>{rightMeta}</span>
      </div>

      <div className="wave-canvas-wrap">
        <canvas
          ref={canvasRef}
          className="gw-canvas"
          height={CANVAS_H}
          onMouseMove={(e) => {
            if (points.length < 2) return;
            const rect = e.currentTarget.getBoundingClientRect();
            const frac = Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width));
            setHoverIdx(Math.round(frac * (points.length - 1)));
          }}
          onMouseLeave={() => setHoverIdx(null)}
        />
        {lastPt && (
          <span
            className="live-beacon"
            style={{
              top: `${beaconY}px`,
              backgroundColor: beaconColor,
              boxShadow: `0 0 10px ${beaconColor}`,
            }}
            aria-hidden="true"
          />
        )}
      </div>

      {rail.length > 0 && (
        <>
          <div className="rail-bar">
            {rail.map((seg) => (
              <div
                key={seg.label}
                style={{
                  width: `${Math.max(1.5, seg.pct)}%`,
                  background: seg.color,
                  transition: "width 280ms cubic-bezier(0.22, 1, 0.36, 1)",
                }}
              />
            ))}
          </div>
          <div className="rail-leg">
            {rail.map((seg) => (
              <span key={seg.label} className="rail-leg-item">
                <i className="rail-dot" style={{ background: seg.color }} />
                {seg.label}
              </span>
            ))}
          </div>
        </>
      )}
    </div>
  );
}
