/// 文件列表的共享模型：快捷面板和桌面格子用的是同一套列目录 / 排序 / 排版参数。
///
/// 只放纯数据和纯函数——后端 `launcher_items_stream` 的返回、排序规则、图标协议、
/// 虚拟滚动的格子尺寸，以及缩放档位。两个页面各自负责渲染和交互。

export type LauncherItem = {
  name: string;
  path: string;
  isDir: boolean;
  size: number;
  modified: number;
  icon: string | null;
};

/// 流式列目录的一页（对应后端的 `LauncherPage`）。
export type LauncherPage = { items: LauncherItem[]; done: boolean };

/// 虚拟滚动的尺寸参数，必须和页面里的网格 / 列表样式一致。
export const GRID_CELL_W = 108;
export const GRID_GAP = 22;
// 48(图标) + 6(间距) + 两行名字(约 32.4) + 上下内边距(18)，留一点余量。
export const GRID_TILE_H = 106;
export const GRID_PAD = 22;
export const LIST_ROW_H = 36;
export const LIST_GAP = 10;
export const LIST_PAD = 10;

/// 文件区排版缩放（Alt+滚轮）的几档：100% 是默认大小，也是最小，只能往大了放。
export const ZOOM_STEPS = [100, 110, 120, 130, 140, 150];
export const ZOOM_DEFAULT = 100;

/// 图标走 Tauri 自定义协议：Windows 上地址是 `http://<scheme>.localhost`。
export const ICON_BASE = "http://hbicon.localhost/";

export type SortKey = "name" | "modified" | "size" | "type" | "frequent";

/// 和资源管理器一致的排序：数字按数值比（`图2 < 图10`）。
/// 资源管理器用的是 Shell 的 StrCmpLogicalW，`numeric` 就是它的等价物。
export const collator = new Intl.Collator("zh-CN", {
  numeric: true,
  sensitivity: "base",
});

export const SORT_LABELS: Record<SortKey, string> = {
  name: "名称",
  modified: "修改时间",
  size: "大小",
  type: "类型",
  frequent: "使用频率",
};

export const SORT_KEYS: SortKey[] = ["name", "modified", "size", "type", "frequent"];

/// 和资源管理器一致的排序：文件夹在前，数字按数值比（`图2 < 图10`）。
/// 「使用频率」按面板里打开过的次数排，用得多（升序时用得少）的在前。
export function sortItems(
  list: LauncherItem[],
  order: { key: SortKey; dir: number },
  usage: Record<string, number> = {},
): LauncherItem[] {
  const sorted = [...list];
  sorted.sort((a, b) => {
    // 使用次数不同的先按次数排：常打开的文件不该被没碰过的文件夹压下去。
    if (order.key === "frequent") {
      const delta = (usage[a.path] ?? 0) - (usage[b.path] ?? 0);
      if (delta !== 0) return delta * order.dir;
      // 次数一样（多数是一次都没打开过的）：文件夹在前、按名字升序。
      // 这里不跟着升降序翻——方向说的是「常用在前 / 不常用在前」，不是名字。
      if (a.isDir !== b.isDir) return a.isDir ? -1 : 1;
      return collator.compare(a.name, b.name);
    }
    // 按修改时间排：文件夹和文件一起排。文件夹也有修改时间，如果还让它
    // 永远压在顶上，「按时间排序」看的就是两段各自排好的列表，不是真在按时间。
    // 时间相同（文件夹常常是同一秒建的）再按名字，免得顺序每刷新一次就变。
    if (order.key === "modified") {
      return (a.modified - b.modified) * order.dir || collator.compare(a.name, b.name);
    }
    // 其余情况文件夹在前，和资源管理器一致。
    if (a.isDir !== b.isDir) return a.isDir ? -1 : 1;
    let result = 0;
    switch (order.key) {
      case "size":
        result = a.size - b.size;
        break;
      case "type":
        result =
          collator.compare(extensionOf(a.name), extensionOf(b.name)) ||
          collator.compare(a.name, b.name);
        break;
      default:
        result = collator.compare(a.name, b.name);
    }
    return result * order.dir;
  });
  return sorted;
}

/// 把任意百分比吸附到最近的一档；比默认还小 / 非法的值一律按 100 算。
export function clampZoom(value: number): number {
  if (!Number.isFinite(value) || value <= ZOOM_DEFAULT) return ZOOM_DEFAULT;
  let best = ZOOM_STEPS[0];
  for (const step of ZOOM_STEPS) {
    if (Math.abs(step - value) < Math.abs(best - value)) best = step;
  }
  return best;
}

/// 沿档位表滚一格，到头就不动。
export function stepZoom(current: number, direction: 1 | -1): number {
  const index = ZOOM_STEPS.indexOf(clampZoom(current));
  const next = Math.min(ZOOM_STEPS.length - 1, Math.max(0, index + direction));
  return ZOOM_STEPS[next];
}

/// 快捷方式不显示后缀，和资源管理器的观感一致。
export function displayName(name: string): string {
  return name.replace(/\.(lnk|url)$/i, "");
}

/// 路径的最后一段，用来当文件夹 / 格子的名字。
export function folderLabel(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const index = Math.max(trimmed.lastIndexOf("\\"), trimmed.lastIndexOf("/"));
  return index >= 0 ? trimmed.slice(index + 1) : trimmed;
}

export function formatSize(bytes: number): string {
  if (bytes <= 0) return "";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${unit === 0 ? value : value.toFixed(value < 10 ? 1 : 0)} ${units[unit]}`;
}

export function formatDate(seconds: number): string {
  if (!seconds) return "";
  const date = new Date(seconds * 1000);
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}/${pad(date.getMonth() + 1)}/${pad(date.getDate())} ${pad(
    date.getHours(),
  )}:${pad(date.getMinutes())}`;
}

export function extensionOf(name: string): string {
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
}
