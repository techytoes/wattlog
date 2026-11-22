import { useEffect, useRef, useState } from "react";

export type LiveMetricMode = "power" | "cpu" | "mem";

export interface RailSegment {
  label: string;
  pct: number;
  color: string;
}

interface ReactiveWaveformProps {
  mode: LiveMetricMode;
  points: number[];
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
  // cpu
  if (v >= 65) return "#F97316";
  if (v >= 35) return "#FBBF24";
  return "#38BDF8";
}

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

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    let rafId = 0;

    const render = (ts: number) => {
      const dpr = window.devicePixelRatio || 1;
      const h = 76;
      if (canvas.width !== Math.floor(width * dpr) || canvas.height !== Math.floor(h * dpr)) {
        canvas.width = Math.floor(width * dpr);
        canvas.height = Math.floor(h * dpr);
        canvas.style.width = `${width}px`;
        canvas.style.height = `${h}px`;
      }

      const ctx = canvas.getContext("2d");
      if (!ctx) return;
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, width, h);

      const safePts = points.length >= 2 ? points : [points[0] ?? 0, points[0] ?? 0];
      const ceiling = Math.max(maxVal, ...safePts.map((p) => p * 1.15), 1);
      const yOf = (v: number) => h - 8 - (Math.max(0, v) / ceiling) * (h - 16);
      const step = width / Math.max(1, safePts.length - 1);

      // Subtle dashed threshold guides
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
        const y = Math.round(h * f) + 0.5;
        ctx.beginPath();
        ctx.moveTo(0, y);
        ctx.lineTo(width, y);
        ctx.stroke();
      });
      ctx.setLineDash([]);

      // Load-reactive vertical stroke & glow gradients
      const sGrad = ctx.createLinearGradient(0, 6, 0, h - 6);
      const aGrad = ctx.createLinearGradient(0, 6, 0, h);
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
        safePts.forEach((v, i) => {
          const x = i * step;
          const y = yOf(v);
          if (i === 0) ctx.moveTo(x, y);
          else {
            const px = (i - 1) * step;
            const py = yOf(safePts[i - 1]);
            const cx = (px + x) / 2;
            ctx.bezierCurveTo(cx, py, cx, y, x, y);
          }
        });
      };

      // Area fill
      trace();
      ctx.lineTo(width, h);
      ctx.lineTo(0, h);
      ctx.closePath();
      ctx.fillStyle = aGrad;
      ctx.fill();

      // Soft OLED bloom
      trace();
      ctx.strokeStyle = sGrad;
      ctx.lineWidth = 5;
      ctx.globalAlpha = 0.2;
      ctx.stroke();

      // Crisp foreground stroke
      trace();
      ctx.globalAlpha = 1;
      ctx.lineWidth = 2.1;
      ctx.stroke();

      // Hover scrub crosshair
      if (hoverIdx !== null && hoverIdx >= 0 && hoverIdx < safePts.length) {
        const hx = hoverIdx * step;
        const hy = yOf(safePts[hoverIdx]);
        ctx.strokeStyle = "rgba(255, 255, 255, 0.22)";
        ctx.lineWidth = 1;
        ctx.beginPath();
        ctx.moveTo(hx, 4);
        ctx.lineTo(hx, h - 4);
        ctx.stroke();

        ctx.beginPath();
        ctx.arc(hx, hy, 4.2, 0, Math.PI * 2);
        ctx.fillStyle = colorForValue(mode, safePts[hoverIdx], ceiling);
        ctx.fill();

        ctx.beginPath();
        ctx.arc(hx, hy, 2, 0, Math.PI * 2);
        ctx.fillStyle = "#FFFFFF";
        ctx.fill();
      }

      // Live pulsing head beacon
      const lastVal = safePts[safePts.length - 1];
      const lastY = yOf(lastVal);
      const headCol = colorForValue(mode, lastVal, ceiling);
      const pulse = 5.5 + Math.sin(ts * 0.004) * 1.5;

      ctx.beginPath();
      ctx.arc(width - 2, lastY, pulse, 0, Math.PI * 2);
      ctx.fillStyle = headCol;
      ctx.globalAlpha = 0.24;
      ctx.fill();
      ctx.globalAlpha = 1;

      ctx.beginPath();
      ctx.arc(width - 2, lastY, 2.6, 0, Math.PI * 2);
      ctx.fillStyle = "#FFFFFF";
      ctx.fill();

      rafId = requestAnimationFrame(render);
    };

    rafId = requestAnimationFrame(render);
    return () => cancelAnimationFrame(rafId);
  }, [width, mode, points, maxVal, hoverIdx]);

  const hoveredVal =
    hoverIdx !== null && hoverIdx >= 0 && hoverIdx < points.length ? points[hoverIdx] : null;
  const hoveredSecsAgo =
    hoverIdx !== null ? Math.round((points.length - 1 - hoverIdx) * 1.5) : 0;

  return (
    <div className="graph-well" ref={wrapRef}>
      <div className="gw-head">
        <span>
          {hoveredVal !== null ? (
            <>
              <strong style={{ color: colorForValue(mode, hoveredVal, maxVal) }}>
                {hoveredVal.toFixed(1)}
                {unit}
              </strong>{" "}
              · {hoveredSecsAgo <= 0 ? "live" : `${hoveredSecsAgo}s ago`}
            </>
          ) : (
            title
          )}
        </span>
        <span>{rightMeta}</span>
      </div>

      <canvas
        ref={canvasRef}
        className="gw-canvas"
        height={76}
        onMouseMove={(e) => {
          const rect = e.currentTarget.getBoundingClientRect();
          const frac = Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width));
          setHoverIdx(Math.round(frac * Math.max(0, points.length - 1)));
        }}
        onMouseLeave={() => setHoverIdx(null)}
      />

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
