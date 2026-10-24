import type { HitDto, MonitorDto, RegionDto, Sides } from "../types";

const WIDTH = 660;
const HEIGHT = 300;
const PAD = 36;

export function MonitorMap({
  monitors,
  regions,
  hit,
  sides,
  dimmed,
}: {
  monitors: MonitorDto[];
  regions: RegionDto[];
  hit: HitDto | null;
  sides: Sides;
  dimmed?: boolean;
}) {
  if (monitors.length === 0) {
    return <div className="callout">正在读取显示器…</div>;
  }

  const minX = Math.min(...monitors.map((m) => m.x));
  const maxX = Math.max(...monitors.map((m) => m.x + m.w));
  const minY = Math.min(...monitors.map((m) => m.y));
  const maxY = Math.max(...monitors.map((m) => m.y + m.h));
  const spanX = maxX - minX;
  const spanY = maxY - minY;
  const scale = Math.min((WIDTH - PAD * 2) / spanX, (HEIGHT - PAD * 2) / spanY);
  const offsetX = PAD + (WIDTH - PAD * 2 - spanX * scale) / 2 - minX * scale;
  const offsetY = PAD + (HEIGHT - PAD * 2 - spanY * scale) / 2 - minY * scale;
  const sx = (x: number) => offsetX + x * scale;
  const sy = (y: number) => offsetY + y * scale;

  return (
    <svg
      className="map"
      viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
      style={{ opacity: dimmed ? 0.55 : 1 }}
      role="img"
      aria-label="显示器布局"
    >
      {monitors.map((monitor) => {
        return (
          <g key={monitor.index}>
            <rect
              className={`monitor ${monitor.primary ? "primary" : ""}`}
              x={sx(monitor.x)}
              y={sy(monitor.y)}
              width={monitor.w * scale}
              height={monitor.h * scale}
              rx={7}
            />
            <text
              className="monitor-label strong"
              x={sx(monitor.x) + 10}
              y={sy(monitor.y) + 18}
            >
              {monitor.primary ? "主屏" : `屏 ${monitor.index + 1}`}
            </text>
            <text
              className="monitor-label"
              x={sx(monitor.x) + 10}
              y={sy(monitor.y) + 33}
            >
              {monitor.w} × {monitor.h}
            </text>
          </g>
        );
      })}

      {regions.map((region, index) => {
        const enabled =
          region.side === "left" ? sides !== "right" : sides !== "left";
        const active =
          hit !== null && hit.monitor === region.monitor && hit.side === region.side;
        const bandWidth = 6;
        const x =
          region.side === "left" ? sx(region.edgeX) : sx(region.edgeX) - bandWidth;
        return (
          <rect
            key={index}
            className={`band ${active ? "hit" : ""} ${enabled ? "" : "off"}`}
            x={x}
            y={sy(region.y0)}
            width={bandWidth}
            height={Math.max(2, (region.y1 - region.y0) * scale)}
            rx={2}
          />
        );
      })}


    </svg>
  );
}
