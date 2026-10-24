import type { MonitorDto, Side, TriggerSettings } from "../types";

const WIDTH = 640;
const HEIGHT = 230;
const PAD = 30;

/** Is this point inside any monitor? Mirrors the Rust probe. */
function covered(monitors: MonitorDto[], x: number, y: number): boolean {
  return monitors.some(
    (m) => x >= m.x && x < m.x + m.w && y >= m.y && y < m.y + m.h,
  );
}

/**
 * Contiguous vertical runs of one monitor edge that are not blocked by another
 * monitor. Same rule as `exposed_segments` in `hexbench-core::geometry`.
 */
function exposedSegments(
  monitors: MonitorDto[],
  monitor: MonitorDto,
  side: Side,
): Array<[number, number]> {
  const probeX = side === "left" ? monitor.x - 1 : monitor.x + monitor.w;
  const cuts = [monitor.y, monitor.y + monitor.h];

  for (const other of monitors) {
    for (const y of [other.y, other.y + other.h]) {
      if (y > monitor.y && y < monitor.y + monitor.h) {
        cuts.push(y);
      }
    }
  }

  cuts.sort((a, b) => a - b);
  const unique = cuts.filter((y, index) => index === 0 || y !== cuts[index - 1]);

  const segments: Array<[number, number]> = [];
  for (let i = 0; i + 1 < unique.length; i++) {
    const a = unique[i];
    const b = unique[i + 1];
    if (b <= a || covered(monitors, probeX, a)) {
      continue;
    }
    const last = segments[segments.length - 1];
    if (last && last[1] === a) {
      last[1] = b;
    } else {
      segments.push([a, b]);
    }
  }
  return segments;
}

export interface PreviewRegion {
  monitor: number;
  side: Side;
  edgeX: number;
  y0: number;
  y1: number;
}

/** The bands a monitor gets from the current position/ratio. */
function bandRanges(
  monitor: MonitorDto,
  trigger: TriggerSettings,
): Array<[number, number]> {
  const height = Math.round(monitor.h * trigger.ratio);
  const top: [number, number] = [monitor.y, monitor.y + height];
  const bottom: [number, number] = [
    monitor.y + monitor.h - height,
    monitor.y + monitor.h,
  ];

  switch (trigger.position) {
    case "top":
      return [top];
    case "bottom":
      return [bottom];
    case "both":
      return top[1] >= bottom[0]
        ? [[monitor.y, monitor.y + monitor.h]]
        : [top, bottom];
  }
}

/** Regions implied by the *edited* config, so the preview reacts instantly. */
export function previewRegions(
  monitors: MonitorDto[],
  trigger: TriggerSettings,
): PreviewRegion[] {
  const regions: PreviewRegion[] = [];

  for (const monitor of monitors) {
    for (const [bandY0, bandY1] of bandRanges(monitor, trigger)) {
      for (const side of ["left", "right"] as const) {
        if (trigger.sides === "left" && side === "right") continue;
        if (trigger.sides === "right" && side === "left") continue;

        const edgeX = side === "left" ? monitor.x : monitor.x + monitor.w - 1;
        for (const [a, b] of exposedSegments(monitors, monitor, side)) {
          const y0 = Math.max(a, bandY0);
          const y1 = Math.min(b, bandY1);
          if (y1 > y0) {
            regions.push({ monitor: monitor.index, side, edgeX, y0, y1 });
          }
        }
      }
    }
  }

  return regions;
}

export function TriggerPreview({
  monitors,
  trigger,
}: {
  monitors: MonitorDto[];
  trigger: TriggerSettings;
}) {
  if (monitors.length === 0) {
    return <div className="callout">正在读取显示器…</div>;
  }

  const regions = previewRegions(monitors, trigger);

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
      role="img"
      aria-label="触发范围预览"
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
            {bandRanges(monitor, trigger).map(([y0, y1], index) => (
              <rect
                key={`band-${index}`}
                className="band-area"
                x={sx(monitor.x)}
                y={sy(y0)}
                width={monitor.w * scale}
                height={Math.max(3, (y1 - y0) * scale)}
                rx={4}
              />
            ))}
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
        const blockWidth = 9;
        const x =
          region.side === "left" ? sx(region.edgeX) : sx(region.edgeX) - blockWidth;
        return (
          <rect
            key={`zone-${index}`}
            className="band zone"
            x={x}
            y={sy(region.y0)}
            width={blockWidth}
            height={Math.max(3, (region.y1 - region.y0) * scale)}
            rx={2}
          />
        );
      })}
    </svg>
  );
}
