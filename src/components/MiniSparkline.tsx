interface MiniSparklineProps {
  points: number[];
  maxVal: number;
  color: string;
}

export function MiniSparkline({ points, maxVal, color }: MiniSparklineProps) {
  const safe = points.length >= 2 ? points.slice(-16) : [points[0] ?? 0, points[0] ?? 0];
  const ceiling = Math.max(maxVal, ...safe, 1);
  const w = 48;
  const h = 15;
  const step = w / Math.max(1, safe.length - 1);

  const d = safe
    .map((v, i) => {
      const x = (i * step).toFixed(1);
      const y = (h - 2 - (Math.max(0, v) / ceiling) * (h - 4)).toFixed(1);
      return `${i === 0 ? "M" : "L"}${x} ${y}`;
    })
    .join(" ");

  return (
    <svg className="mt-spark" viewBox={`0 0 ${w} ${h}`} aria-hidden="true">
      <path d={d} fill="none" stroke={color} strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}
