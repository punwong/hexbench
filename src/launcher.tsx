import "./shell";

import React, {
  Fragment,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import ReactDOM from "react-dom/client";
import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

/** "top:0;left:8px" → { top: "0", left: "8px" }，省得写 8 组 style 对象。 */
function cssFrom(source: string): Record<string, string> {
  return Object.fromEntries(
    source
      .split(";")
      .filter(Boolean)
      .map((pair) => {
        const [name, value] = pair.split(":");
        return [name.replace(/-(\w)/g, (_, letter: string) => letter.toUpperCase()), value];
      }),
  );
}

type LauncherItem = {
  name: string;
  path: string;
  isDir: boolean;
  size: number;
  modified: number;
  icon: string | null;
};

/// 流式列目录的一页（对应后端的 `LauncherPage`）。
type LauncherPage = { items: LauncherItem[]; done: boolean };

/// 虚拟滚动的尺寸参数，必须和 `launcher.html` 里的网格/列表样式一致。
const GRID_CELL_W = 108;
const GRID_GAP = 22;
// 48(图标) + 6(间距) + 两行名字(约 32.4) + 上下内边距(18)，留一点余量。
const GRID_TILE_H = 106;
const GRID_PAD = 22;
const LIST_ROW_H = 36;
const LIST_GAP = 10;
const LIST_PAD = 10;
/// 图标走 Tauri 自定义协议：Windows 上地址是 `http://<scheme>.localhost`。
const ICON_BASE = "http://hbicon.localhost/";
/// 除了屏幕上可见的，再多要一屏多的图标：滚下去的时候已经准备好了。
const ICON_LOOKAHEAD = 96;
/// 拖拽预览用的预热：侧边栏每个文件夹备头一屏的图标。
const PREVIEW_WARM = 32;

type Category = { name: string; path: string; count: number; hidden: boolean };
type LauncherRoot = {
  root: string;
  /** 「全部」要不要出现在侧边栏 */
  allVisible: boolean;
  categories: Category[];
  /** 常用文件夹，顺序就是侧边栏顺序 */
  folders: string[];
  /** 网址分组 */
  sites: Array<{ name: string; url: string }>;
  /** 设置里的「内置网页」开关：开着就默认展开右侧栏 */
  webEnabled: boolean;
  view: string;
  sortKey: string;
  sortDir: number;
  /** 快速访问那一组单独记的视图 / 排序 */
  pinnedView: string;
  pinnedSortKey: string;
  pinnedSortDir: number;
  /** 快速访问是否已经单独设过；没设过就跟着分类那套走 */
  pinnedViewSet: boolean;
  rememberLocation: boolean;
  lastKind: string;
  lastBase: string;
  lastLabel: string;
  lastPath: string;
  /** 上次停在「快速访问」那一行 */
  lastPinned: boolean;
};

type SortKey = "name" | "modified" | "size" | "type" | "frequent";

/// 和资源管理器一致的排序：数字按数值比（`图2 < 图10`）。
/// 资源管理器用的是 Shell 的 StrCmpLogicalW，`numeric` 就是它的等价物。
const collator = new Intl.Collator("zh-CN", { numeric: true, sensitivity: "base" });

const SORT_LABELS: Record<SortKey, string> = {
  name: "名称",
  modified: "修改时间",
  size: "大小",
  type: "类型",
  frequent: "使用频率",
};

/// 和资源管理器一致的排序：文件夹在前，数字按数值比（`图2 < 图10`）。
/// 「使用频率」按面板里打开过的次数排，用得多（升序时用得少）的在前。
function sortItems(
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
    // 其余情况文件夹在前，和资源管理器一致。
    if (a.isDir !== b.isDir) return a.isDir ? -1 : 1;
    let result = 0;
    switch (order.key) {
      case "modified":
        result = a.modified - b.modified;
        break;
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

/** 面板现在浏览的是哪一片区域。 */
type Scope =
  | { kind: "all" }
  /** 一棵真实的目录树：某个分类，或者一个常用文件夹。 */
  | { kind: "folder"; base: string; label: string; pinned?: boolean };

/** 快捷方式不显示后缀，和资源管理器的观感一致。 */
function displayName(name: string): string {
  return name.replace(/\.(lnk|url)$/i, "");
}

/** 路径的最后一段，用来当常用文件夹的名字。 */
function folderLabel(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const index = Math.max(trimmed.lastIndexOf("\\"), trimmed.lastIndexOf("/"));
  return index >= 0 ? trimmed.slice(index + 1) : trimmed;
}

type Snapshot = { scope: Scope; path: string };

/** 面板的显示方式。快速启动（分类）和快速访问（钉住的文件夹）各记一套。 */
type ViewPref = { view: "grid" | "list"; sort: { key: SortKey; dir: 1 | -1 } };

const SORT_KEYS: SortKey[] = ["name", "modified", "size", "type", "frequent"];

/** 把后端存的字符串读成一套能直接用的显示方式（不认识的字段退回默认）。 */
function readPref(view: string, sortKey: string, sortDir: number): ViewPref {
  const key = SORT_KEYS.find((entry) => entry === sortKey) ?? "name";
  return {
    view: view === "list" ? "list" : "grid",
    sort: { key, dir: sortDir === -1 ? -1 : 1 },
  };
}

function normalize(path: string): string {
  return path.replace(/\//g, "\\").replace(/\\+$/, "").toLowerCase();
}

function isInside(child: string, parent: string): boolean {
  if (!parent) return false;
  const a = normalize(child);
  const b = normalize(parent);
  return a === b || a.startsWith(`${b}\\`);
}

function parentOf(path: string): string {
  const trimmed = path.replace(/\\+$/, "");
  const cut = trimmed.lastIndexOf("\\");
  if (cut <= 2) return trimmed;
  return trimmed.slice(0, cut);
}

function formatSize(bytes: number): string {
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

function formatDate(seconds: number): string {
  if (!seconds) return "";
  const date = new Date(seconds * 1000);
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}/${pad(date.getMonth() + 1)}/${pad(date.getDate())} ${pad(
    date.getHours(),
  )}:${pad(date.getMinutes())}`;
}

function extensionOf(name: string): string {
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
}

/**
 * 呼出面板。
 *
 * 布局跟主设置窗口一致：左侧一整条导航，右侧上方是「前进 / 后退 / 上一级 +
 * 面包屑 + 排序」，下面是网格，底部状态。分类、全部、中转站共用同一套浏览
 * 模型：双击文件夹进去，面包屑点任意一节跳回那一层。
 */
function Launcher() {
  const inputRef = useRef<HTMLInputElement>(null);
  const gridRef = useRef<HTMLDivElement>(null);

  // 数据
  const [root, setRoot] = useState<LauncherRoot>({
    root: "",
    allVisible: false,
    categories: [],
    folders: [],
    sites: [],
    webEnabled: false,
    view: "grid",
    sortKey: "name",
    sortDir: 1,
    pinnedView: "grid",
    pinnedSortKey: "name",
    pinnedSortDir: 1,
    pinnedViewSet: false,
    rememberLocation: false,
    lastKind: "",
    lastBase: "",
    lastLabel: "",
    lastPath: "",
    lastPinned: false,
  });
  const [allCache, setAllCache] = useState<Record<string, LauncherItem[]>>({});
  const [items, setItems] = useState<LauncherItem[]>([]);
  const [loading, setLoading] = useState(false);
  // 图标单独存：先出名字，图标随后分批补，切目录时不会串。
  const [iconByPath, setIconByPath] = useState<Record<string, string | null>>({});
  const iconRef = useRef<Record<string, string | null>>({});
  // 已经淡入过的图标：滚动把它重新挂出来时直接给成品，别又淡一次。
  const iconReady = useRef<Set<string>>(new Set());
  const viewToken = useRef(0);
  // 虚拟滚动：只渲染视口里的几行，几千项的目录也不会把 DOM 撑爆。
  const [viewport, setViewport] = useState({ width: 0, height: 0, scrollTop: 0 });

  // 图标现在只回一个缓存键，真正的 PNG 由 `hbicon://` 协议直接给 WebView。
  // 已经把请求发出去、还没等到结果的路径：可见区、预取和滚动会重叠，
  // 没有这层去重，同一张图会被反复要。可见的和预取的分开记：预取过的
  // 一旦滚到眼前，还要能再要一次，让后端把它挪到队首。
  const pendingIcons = useRef<Set<string>>(new Set());
  const pendingAhead = useRef<Set<string>>(new Set());
  const flushIcons = useRef<number | null>(null);

  // 拖拽期间主线程被 DoDragDrop 的模态循环占着：`hbicon://` 的新请求要等松手才受理，
  // 取图标的命令也一样。所以那期间才显示的一屏缩略图，得现在就已经在渲染进程里。
  // 空闲时把侧边栏每个文件夹头一屏的图标都取进来，Image 对象一直持有、不放给 GC，
  // 拖着文件悬停过去时预览直接出图（内存缓存里就有，不必再请求）。
  const warmPaths = useRef<Set<string>>(new Set());
  const warmImages = useRef<HTMLImageElement[]>([]);
  const warmIcon = useCallback((path: string, key: string | null) => {
    if (!key || !warmPaths.current.has(path)) return;
    // 侧边栏再长也封顶，别让常驻面板一直涨内存。
    if (warmImages.current.length >= PREVIEW_WARM * 40) return;
    const image = new Image();
    image.src = ICON_BASE + key;
    warmImages.current.push(image);
  }, []);

  const pushIcon = useCallback((path: string, key: string | null) => {
    pendingIcons.current.delete(path);
    pendingAhead.current.delete(path);
    if (path in iconRef.current && iconRef.current[path] === key) return;
    iconRef.current[path] = key;
    warmIcon(path, key);
    // 一屏几十张图一起回来时，别每张都触发一次整表复制 + 渲染。
    if (flushIcons.current !== null) return;
    flushIcons.current = window.setTimeout(() => {
      flushIcons.current = null;
      setIconByPath({ ...iconRef.current });
    }, 60);
  }, [warmIcon]);

  // `visible` 是这一屏，`ahead` 是往后预取的一截：后端先抽可见的，
  // 滚得再快，新一屏也不用等前面的预取。
  const fillIcons = useCallback(
    (visible: string[], ahead: string[] = []) => {
      if (!visible.length && !ahead.length) return;
      for (const path of visible) pendingIcons.current.add(path);
      for (const path of ahead) pendingAhead.current.add(path);
      void invoke<Array<[string, string | null]>>("launcher_icons", { paths: visible, ahead })
        .then((ready) => {
          // 缓存命中的随返回值直接回来；没命中的后端在后台抽，
          // 每抽好一张发一条 `launcher-icon`，不用等整批。
          for (const [path, key] of ready) pushIcon(path, key);
        })
        .catch(() => {
          for (const path of visible) pendingIcons.current.delete(path);
          for (const path of ahead) pendingAhead.current.delete(path);
        });
    },
    [pushIcon],
  );

  // 后端的后台抽取每完成一张就发一条。按路径存，切目录也不会串。
  useEffect(() => {
    let alive = true;
    const stop = listen<{ path: string; key: string | null }>("launcher-icon", (event) => {
      if (alive) pushIcon(event.payload.path, event.payload.key);
    });
    return () => {
      alive = false;
      void stop.then((unlisten) => unlisten());
    };
  }, [pushIcon]);

  // 每次呼出都重播一次入场：窗口是常驻的，页面的 CSS 动画只在加载时跑一次，
  // 不重挂的话第一回之后就是「啪」地出现。后端 reveal 完发 launcher-shown。
  const paletteRef = useRef<HTMLDivElement | null>(null);
  const playEnter = useCallback(() => {
    const element = paletteRef.current;
    if (!element) return;
    // 系统里关了动画就什么都不做，面板直接出现。
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    // 用 Web Animations 而不是切换 CSS 类：摘类的那一帧会露出完全不透明的
    // 面板，再淡入就是「闪一下」。这里直接从 0 开始。
    element.animate([{ opacity: 0 }, { opacity: 1 }], {
      duration: 140,
      easing: "cubic-bezier(0.2, 0, 0, 1)",
    });
  }, []);
  useLayoutEffect(() => {
    playEnter();
    let alive = true;
    const stop = listen("launcher-shown", () => {
      if (alive) playEnter();
    });
    return () => {
      alive = false;
      void stop.then((unlisten) => unlisten());
    };
  }, [playEnter]);

  // 导航
  const [scope, setScope] = useState<Scope>({ kind: "all" });
  const [path, setPath] = useState("");
  const [backStack, setBackStack] = useState<Snapshot[]>([]);
  const [forwardStack, setForwardStack] = useState<Snapshot[]>([]);

  // 视图
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState(-1);
  // 多选：picked 是真正被选中的路径集合，selected 只是键盘/悬停的光标。
  const [picked, setPicked] = useState<Set<string>>(() => new Set());
  // 拖拽悬停在哪一行/格子上（只对文件夹有意义）
  const [dropTarget, setDropTarget] = useState<string | null>(null);
  // 拖着文件压在面包屑上：高亮那一段，松手就移进那一层。
  const [dropCrumb, setDropCrumb] = useState<string | null>(null);
  // 正在面板里看的内嵌网页；null = 文件视图
  // 站点图标：抓一次就缓存，标签页直接用。
  const [siteIcons, setSiteIcons] = useState<Record<string, string>>({});
  // 侧边栏右键菜单 + 内联输入（新建 / 重命名分类）
  const [sideMenu, setSideMenu] = useState<{
    x: number;
    y: number;
    path: string;
    label: string;
  } | null>(null);
  const [sideInput, setSideInput] = useState<{
    kind: "new" | "rename";
    path: string;
    value: string;
  } | null>(null);
  // 搬完文件逼加载器重跑一次：目录内容已经变了。
  const [loadTick, setLoadTick] = useState(0);
  // 顶部飘一句提示：改名/移动失败时得让人看见原因。
  const [toast, setToast] = useState<string | null>(null);
  const toastTimer = useRef<number>(0);
  const showToast = useCallback((message: string) => {
    setToast(message);
    window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(null), 2800);
  }, []);
  const anchor = useRef(-1);
  const [marquee, setMarquee] = useState<{
    left: number;
    top: number;
    width: number;
    height: number;
  } | null>(null);
  const marqueeRef = useRef<{ x: number; y: number; base: Set<string> } | null>(null);
  const [sort, setSort] = useState<{ key: SortKey; dir: 1 | -1 }>({ key: "name", dir: 1 });
  const [view, setView] = useState<"grid" | "list">("grid");
  // 每个文件在面板里被打开过几次，「使用频率」排序按它来。
  const [usage, setUsage] = useState<Record<string, number>>({});
  // 两份显示方式：切到哪一组就套哪一份，改的时候也只记回自己那份。
  const viewPrefs = useRef<{ plain: ViewPref; pinned: ViewPref }>({
    plain: { view: "grid", sort: { key: "name", dir: 1 } },
    pinned: { view: "grid", sort: { key: "name", dir: 1 } },
  });
  // 快速访问那套是不是已经单独设过：没设过就先跟着分类走。
  const pinnedViewSet = useRef(false);
  const [sortMenu, setSortMenu] = useState(false);

  // 视口尺寸变化（切网格/列表、窗口拉大、滚动条出现）都要重新算可视范围。
  useEffect(() => {
    const element = gridRef.current;
    if (!element) return;
    const measure = () =>
      setViewport((previous) => {
        const next = {
          width: element.clientWidth,
          height: element.clientHeight,
          scrollTop: element.scrollTop,
        };
        return previous.width === next.width &&
          previous.height === next.height &&
          previous.scrollTop === next.scrollTop
          ? previous
          : next;
      });
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, [view]);

  const hoverArmed = useRef(false);
  const [editing, setEditing] = useState<{ path: string; name: string } | null>(null);
  const skipCommit = useRef(false);
  const renameInputRef = useRef<HTMLTextAreaElement | null>(null);

  // 拖放回调是在挂载时注册的，读不到最新状态，用 ref 兜住。
  const shownRef = useRef<LauncherItem[]>([]);
  const currentDirRef = useRef("");
  const refreshRef = useRef<() => void>(() => {});
  const showToastRef = useRef<(message: string) => void>(() => {});
  // 拖拽时回调闭包里读不到最新状态：把「当前分类」和「点某个分类要做什么」放进来。
  const activeIdRef = useRef("__all__");
  // 拖拽进行中：主线程被 DoDragDrop 占着，取目录的命令发不出去。
  const dragActiveRef = useRef(false);
  // 最后一次收到拖拽坐标的时间：拖拽结束的通知偶尔会丢（拖到窗口外松手、
  // 拖拽被系统取消），只看布尔值会让面板一直不刷新，所以要能自己过期。
  const dragSeenAt = useRef(0);
  const sidesRef = useRef<Record<string, () => void>>({});
  // 刚处理过的落点：同一次拖放可能被两条路径报回来两次，只认第一次。
  const lastDropRef = useRef({ key: "", at: 0 });
  const base = scope.kind === "folder" ? scope.base : "";
  const currentDir = scope.kind === "folder" ? path || scope.base : root.root;

  const reload = useCallback(async (dropStale = false): Promise<LauncherRoot | null> => {
    const listing = await invoke<LauncherRoot>("launcher_categories");
    setRoot(listing);
    const loaded: Record<string, LauncherItem[]> = {};
    if (listing.categories.length <= 12) {
      for (const category of listing.categories) {
        loaded[category.path] = await invoke<LauncherItem[]>("launcher_items", {
          path: category.path,
          withIcons: false,
        });
      }
    }
    // 分类的列表在这里重读了一遍；钉住的文件夹（以及预读过的其它目录）不在
    // 分类里，上面没碰它们。文件被移动 / 粘贴 / 新建之后，缓存里的旧列表
    // 不能再拿去画界面了——目标目录里明明已经有文件，面板却显示「这里是空的」。
    // 丢掉这些条目，让读取重新来一遍；纯呼出时保留，首屏才不用等。
    setAllCache(dropStale ? () => loaded : (previous) => ({ ...previous, ...loaded }));
    return listing;
  }, []);

  /** 回到上次停留的位置；没记住就回到「全部」。 */
  const restore = useCallback(
    (listing: LauncherRoot | null) => {
      // 上次的位置还得存在：分类换过 / 目录删了，就退回「全部」。
      // 这里必须用接口刚返回的数据，而不是组件状态——调用这一刻状态还是旧的。
      const base = listing?.lastBase ?? "";
      const known =
        !!base &&
        (listing?.categories.some((category) => category.path === base) ||
          (listing?.folders ?? []).includes(base));
      if (listing) {
        const plain = readPref(listing.view, listing.sortKey, listing.sortDir);
        viewPrefs.current = {
          plain,
          pinned: readPref(listing.pinnedView, listing.pinnedSortKey, listing.pinnedSortDir),
        };
        pinnedViewSet.current = listing.pinnedViewSet;
        // 上次停在快速访问里就套那一套，否则用分类的。
        const pinnedRestore =
          !!listing.rememberLocation &&
          listing.lastKind === "folder" &&
          known &&
          !!listing.lastPinned &&
          (listing.folders ?? []).includes(base);
        const pref =
          pinnedRestore && pinnedViewSet.current ? viewPrefs.current.pinned : plain;
        setView(pref.view);
        setSort(pref.sort);
      }
      if (listing?.rememberLocation && listing.lastKind === "folder" && known) {
        // 同一个文件夹可能既是分类又是快速访问：按上次停在哪一行还原，
        // 否则高亮会落到另一边。
        const pinned = !!listing.lastPinned && (listing.folders ?? []).includes(base);
        setScope({
          kind: "folder",
          base,
          label: listing.lastLabel || "文件夹",
          pinned: pinned || undefined,
        });
        setPath(listing.lastPath || base);
        return;
      }
      setScope({ kind: "all" });
      setPath("");
    },
    [],
  );

  useEffect(() => {
    void reload().then(restore);
  }, [reload, restore]);


  // 拖拽到面板上：高亮光标下的文件夹，松手就移进去。
  useEffect(() => {
    const disposers: Array<() => void> = [];
    let disposed = false;
    /**
     * 注册监听；effect 已经拆掉的话，监听一到手就注销。
     *
     * `listen` 是异步的：cleanup 常在这个 promise 返回之前就跑完了，那时
     * disposers 还是空的，监听就留在页面里。开发模式下 effect 会连着跑两遍，
     * 于是同一次拖放被处理两次——第二次去移动已经挪走的文件，
     * 就弹出「文件已经不在了」。
     */
    const on = <T,>(name: string, handler: (event: { payload: T }) => void) => {
      void listen<T>(name, handler).then((fn) => {
        if (disposed) fn();
        else disposers.push(fn);
      });
    };
    // 面板自己的拖放目标收不到 OLE 事件，光标位置由后端轮询发过来。
    const toClient = (x: number, y: number) => {
      const ratio = window.devicePixelRatio || 1;
      return { x: x / ratio - window.screenX, y: y / ratio - window.screenY };
    };
    const folderAtClient = (x: number, y: number) => {
      const node = document.elementFromPoint(x, y);
      const row = node?.closest<HTMLElement>("[data-index]");
      if (!row) return null;
      const item = shownRef.current[Number(row.dataset.index)];
      return item && item.isDir ? item : null;
    };
    /** 光标下面是侧边栏的哪一项（id 用来比高亮，path 用来当落点）。 */
    const sideAtClient = (x: number, y: number): { id: string; path: string } | null => {
      const node = document.elementFromPoint(x, y);
      const side = node?.closest<HTMLElement>("[data-side]");
      if (!side?.dataset.side) return null;
      return { id: side.dataset.side, path: side.dataset.path ?? "" };
    };
    /** 光标压在面包屑的哪一段上：那是要移进去的上一级目录。 */
    const crumbAtClient = (x: number, y: number): string | null => {
      const node = document.elementFromPoint(x, y);
      return node?.closest<HTMLElement>("[data-crumb]")?.dataset.crumb ?? null;
    };
    /** 松手：落在侧边栏分类或文件夹格子上就移进去，落在空白就放进当前目录。 */
    const dropInto = (x: number, y: number, paths: string[]) => {
      // 同一次拖放有可能被两条路径报回来（拖拽结束时的兜底判断、面板自己的
      // 拖放目标）。只认第一次：第二次的源文件已经不在了，会白弹一个错误。
      const key = paths.join("|");
      const now = Date.now();
      if (key === lastDropRef.current.key && now - lastDropRef.current.at < 800) return;
      lastDropRef.current = { key, at: now };
      dragActiveRef.current = false;
      setLoadTick((tick) => tick + 1);
      setDropTarget(null);
      setDropCrumb(null);
      const point = toClient(x, y);
      const folder = folderAtClient(point.x, point.y);
      const side = sideAtClient(point.x, point.y);
      // 面包屑优先：拖到「上一级」那一段上松手，就移进那一层。
      const dest =
        crumbAtClient(point.x, point.y) || side?.path || folder?.path || currentDirRef.current;
      if (!dest) return;
      void invoke("launcher_move_into", { dest, paths })
        .then(() => refreshRef.current())
        .catch((error) => showToastRef.current(String(error)));
    };
    on<{ x: number; y: number }>("launcher-drag-pos", (event) => {
      dragActiveRef.current = true;
      dragSeenAt.current = Date.now();
      const point = toClient(event.payload.x, event.payload.y);
      setDropTarget(folderAtClient(point.x, point.y)?.path ?? null);
      setDropCrumb(crumbAtClient(point.x, point.y));
      // 拖着东西悬到分类上：切过去，这样才落得进去。
      // 拖拽期间浏览器不发 mouseenter，只能靠后端轮询来的坐标自己判断。
      const side = sideAtClient(point.x, point.y);
      if (side && side.id !== activeIdRef.current) {
        const switchTo = sidesRef.current[side.id];
        if (switchTo) switchTo();
      }
    });
    on<{ x: number; y: number; paths: string[] }>("launcher-drag-drop", (event) =>
      dropInto(event.payload.x, event.payload.y, event.payload.paths),
    );
    // 后端收起面板（切到别的应用）时，把网页状态一起清掉。
    on<null>("launcher-drag-leave", () => {
      dragActiveRef.current = false;
      setLoadTick((tick) => tick + 1);
      setDropTarget(null);
      setDropCrumb(null);
    });
    on<{ x: number; y: number; paths: string[] }>("launcher-dropped", (event) =>
      dropInto(event.payload.x, event.payload.y, event.payload.paths),
    );
    return () => {
      disposed = true;
      disposers.forEach((fn) => fn());
    };
  }, []);

  // 视图 / 排序变化也记下来，只记回当前这一组：快速启动和快速访问分开记。
  const pinnedScope = scope.kind === "folder" && !!scope.pinned;
  useEffect(() => {
    viewPrefs.current[pinnedScope ? "pinned" : "plain"] = { view, sort };
    if (pinnedScope) pinnedViewSet.current = true;
    const timer = window.setTimeout(() => {
      void invoke("launcher_save_view", {
        view,
        sortKey: sort.key,
        sortDir: sort.dir,
        pinned: pinnedScope,
      }).catch(() => {});
    }, 400);
    return () => window.clearTimeout(timer);
  }, [view, sort.key, sort.dir, pinnedScope]);

  // 位置变化就记下来（防抖，避免每次点击都写盘）。
  useEffect(() => {
    if (!root.rememberLocation) return;
    const timer = window.setTimeout(() => {
      void invoke("launcher_save_location", {
        kind: scope.kind,
        base: scope.kind === "folder" ? scope.base : "",
        label: scope.kind === "folder" ? scope.label : "",
        path: scope.kind === "folder" ? path : "",
        pinned: scope.kind === "folder" ? !!scope.pinned : false,
      });
    }, 400);
    return () => window.clearTimeout(timer);
  }, [scope, path, root.rememberLocation]);

  // 读当前这一层：名字流式推过来，图标按可见范围再补。
  useEffect(() => {
    let alive = true;
    viewToken.current += 1;
    const token = viewToken.current;
    // 「这次没取到」只算这一次：切目录、刷新时清掉，下次可见再问一遍 Shell，
    // 别让一次卡顿变成永久回退。
    for (const [iconPath, key] of Object.entries(iconRef.current)) {
      if (key === null) delete iconRef.current[iconPath];
    }
    const run = async () => {
      if (scope.kind === "all" && !path) {
        const all = root.categories.flatMap((category) => allCache[category.path] ?? []);
        if (!alive) return;
        setItems(all);
        setLoading(false);
        return;
      }
      const target = path || (scope.kind === "folder" ? scope.base : "");
      if (!target) {
        setItems([]);
        setLoading(false);
        return;
      }
      // 预读过的分类直接用缓存：拖拽期间主线程被 DoDragDrop 占着，
      // 这时候发命令只会一直卡在「正在读取」。
      const cached = allCache[target];
      if (cached) {
        setItems(cached);
        setLoading(false);
        // 和下面现读一样从顶上开始：换了目录还停在上一个目录的滚动位置上很怪，
        // 而且拖拽预览的缩略图只备了头一屏。
        if (gridRef.current) gridRef.current.scrollTop = 0;
        return;
      }
      // 没缓存又正在拖：保持原样，别转圈，松手后再补。
      // 1.5 秒没有新的拖拽坐标就当拖拽已经结束——拖拽结束事件丢了的时候
      // （拖到窗口外松手、被系统取消）不能一直卡在这里。
      if (dragActiveRef.current && Date.now() - dragSeenAt.current < 1500) {
        setLoading(false);
        return;
      }
      // 先清空：否则切目录时上一个目录的内容会留在屏幕上，搜索就会「搜到别处」。
      setItems([]);
      setLoading(true);
      if (gridRef.current) gridRef.current.scrollTop = 0;
      const channel = new Channel<LauncherPage>();
      channel.onmessage = (page) => {
        if (!alive || token !== viewToken.current) return;
        if (page.items.length) {
          setItems((previous) => [...previous, ...page.items]);
        }
        if (page.done) setLoading(false);
      };
      try {
        await invoke("launcher_items_stream", { path: target, onPage: channel });
      } catch (error) {
        if (alive && token === viewToken.current) showToastRef.current(String(error));
      } finally {
        if (alive && token === viewToken.current) setLoading(false);
      }
    };
    void run();
    return () => {
      alive = false;
    };
  }, [scope, path, root.categories, allCache, loadTick]);

  // 分类多到没预读全时，搜索期间把缺的补上：不能因为没预读就少搜几个分类。
  useEffect(() => {
    if (!query.trim()) return;
    const missing = root.categories
      .map((category) => category.path)
      .filter((path) => !(path in allCache));
    if (missing.length === 0) return;
    let alive = true;
    void (async () => {
      for (const path of missing) {
        try {
          const list = await invoke<LauncherItem[]>("launcher_items", {
            path,
            withIcons: false,
          });
          if (!alive) return;
          setAllCache((previous) => ({ ...previous, [path]: list }));
        } catch {
          // 读不了就跳过：少一个分类，别把搜索卡住。
        }
      }
    })();
    return () => {
      alive = false;
    };
  }, [query, root.categories, allCache]);

  // 站点图标：每个地址抓一次，抓不到就退化成一个字。
  useEffect(() => {
    let alive = true;
    for (const entry of root.sites) {
      if (!entry.url || siteIcons[entry.url]) continue;
      void invoke<string>("site_icon", { url: entry.url })
        .then((icon) => {
          if (!alive || !icon) return;
          setSiteIcons((previous) => ({ ...previous, [entry.url]: icon }));
        })
        .catch(() => {});
    }
    return () => {
      alive = false;
    };
  }, [root.sites, siteIcons]);

  // 空白处右键：弹资源管理器的「文件夹背景菜单」（新建 / 粘贴 / 属性 / 刷新）。
  useEffect(() => {
    const handler = (event: MouseEvent) => {
      const target = event.target as HTMLElement | null;
      if (!target || !target.closest(".content")) return;
      if (target.closest(".tile, .row")) return;
      const dir = currentDir || root.root;
      if (!dir) return;
      event.preventDefault();
      void invoke("launcher_background_menu", { path: dir }).catch((error) =>
        showToastRef.current(String(error)),
      );
    };
    document.addEventListener("contextmenu", handler);
    return () => document.removeEventListener("contextmenu", handler);
  }, [currentDir, root.root]);
  // 拖到面板上的文件放进当前浏览的目录。
  useEffect(() => {
    currentDirRef.current = currentDir;
    void invoke("launcher_set_drop_target", { path: currentDir || null }).catch(() => {});
  }, [currentDir]);

  // 文件夹变化 / 面板重新呼出：刷新。
  useEffect(() => {
    const disposers: Array<() => void> = [];
    // 目录内容变了：缓存里钉住的文件夹也要作废（见 reload 的注释）。
    void listen("launcher-changed", () => void reload(true)).then((fn) => disposers.push(fn));
    void listen("launcher-shown", () => {
      // 每次呼出都以后端为准，避免「第一次收了、第二次就变不回来」。
      void invoke<boolean>("web_side_state")
        .then((open) => document.body.classList.toggle("web-open", open))
        .catch(() => {});
      setQuery("");
      setSelected(-1);
      setPicked(new Set());
      anchor.current = -1;
      setBackStack([]);
      setForwardStack([]);
      hoverArmed.current = false;
      void reload().then(restore);
    }).then((fn) => disposers.push(fn));
    return () => disposers.forEach((fn) => fn());
  }, [reload]);

  // 排序菜单：点别处就关掉。
  useEffect(() => {
    if (!sortMenu) return;
    const close = (event: PointerEvent) => {
      if ((event.target as HTMLElement).closest(".sort-wrap")) return;
      setSortMenu(false);
    };
    window.addEventListener("pointerdown", close);
    return () => window.removeEventListener("pointerdown", close);
  }, [sortMenu]);

  // 改名进行中的标记：窗口重新获得焦点时不能把焦点抢回搜索框。
  const renamingRef = useRef(false);
  useEffect(() => {
    renamingRef.current = editing !== null;
  }, [editing]);
  // 改名框出现时聚焦并选中主名（扩展名不选）。放在 effect 里而不是 ref 回调里：
  // ref 回调每次渲染都会重新执行，那样每敲一个字都会重新全选。
  useEffect(() => {
    if (!editing) return;
    const focus = () => {
      const node = renameInputRef.current;
      if (!node) return;
      node.focus();
      const dot = node.value.lastIndexOf(".");
      const end = dot > 0 ? dot : node.value.length;
      node.setSelectionRange(0, end);
    };
    focus();
    // 原生菜单关掉之后窗口的键盘焦点是异步回来的，多补几次。
    const timers = [60, 160, 320].map((delay) => window.setTimeout(focus, delay));
    return () => timers.forEach((timer) => window.clearTimeout(timer));
  }, [editing?.path]);

  // 每次呼出都能直接打字。
  useEffect(() => {
    const focusInput = () => {
      // 正在改名时别把焦点抢走：直接看当前焦点在不在文本域上。
      if (document.activeElement?.tagName === "TEXTAREA") return;
      inputRef.current?.focus();
    };
    focusInput();
    window.addEventListener("focus", focusInput);
    return () => window.removeEventListener("focus", focusInput);
  }, []);

  // ---------- 导航 ----------

  const resetView = useCallback(() => {
    setQuery("");
    setSelected(-1);
    setPicked(new Set());
    anchor.current = -1;
    hoverArmed.current = false;
    requestAnimationFrame(() => inputRef.current?.focus());
  }, []);

  /**
   * 换组时套上那一组自己记住的显示方式：快速启动一套、快速访问一套。
   * 同一组内部走动（进子文件夹、上下级）套的还是同一份，等于没变。
   */
  const applyView = useCallback((next: Scope) => {
    const pinned = next.kind === "folder" && !!next.pinned;
    const pref =
      pinned && pinnedViewSet.current ? viewPrefs.current.pinned : viewPrefs.current.plain;
    setView(pref.view);
    setSort(pref.sort);
  }, []);

  const go = useCallback(
    (next: Snapshot) => {
      // 切回文件视图时把内嵌网页关掉。
      setBackStack((previous) => [...previous, { scope, path }]);
      setForwardStack([]);
      setScope(next.scope);
      setPath(next.path);
      applyView(next.scope);
      resetView();
    },
    [scope, path, resetView, applyView],
  );

  const goBack = useCallback(() => {
    const target = backStack[backStack.length - 1];
    if (!target) return;
    setBackStack((previous) => previous.slice(0, -1));
    setForwardStack((forward) => [...forward, { scope, path }]);
    setScope(target.scope);
    setPath(target.path);
    applyView(target.scope);
    resetView();
  }, [backStack, scope, path, resetView, applyView]);

  const goForward = useCallback(() => {
    const target = forwardStack[forwardStack.length - 1];
    if (!target) return;
    setForwardStack((previous) => previous.slice(0, -1));
    setBackStack((back) => [...back, { scope, path }]);
    setScope(target.scope);
    setPath(target.path);
    applyView(target.scope);
    resetView();
  }, [forwardStack, scope, path, resetView, applyView]);

  const canUp = scope.kind === "folder" && !!path && path !== base;

  const goUp = useCallback(() => {
    if (scope.kind !== "folder" || !path || path === scope.base) return;
    go({ scope, path: parentOf(path) });
  }, [go, scope, path]);

  const enterFolder = useCallback(
    (folderPath: string) => {
      // 搜索结果是跨分类的：进别的分类里的文件夹要连分类一起切过去，
      // 否则面包屑和「上一层」都会指错地方。
      if (scope.kind === "all" || !isInside(folderPath, scope.base)) {
        const owner = root.categories.find((category) => isInside(folderPath, category.path));
        if (owner) {
          go({ scope: { kind: "folder", base: owner.path, label: owner.name }, path: folderPath });
          return;
        }
        const pin = root.folders.find((folder) => isInside(folderPath, folder));
        if (pin) {
          go({
            scope: { kind: "folder", base: pin, label: folderLabel(pin), pinned: true },
            path: folderPath,
          });
          return;
        }
      }
      go({ scope, path: folderPath });
    },
    [go, scope, root.categories, root.folders],
  );

  const openCategory = useCallback(
    (category: Category) => {
      go({ scope: { kind: "folder", base: category.path, label: category.name }, path: category.path });
    },
    [go],
  );

  const openAll = useCallback(() => go({ scope: { kind: "all" }, path: "" }), [go]);


  const openPinned = useCallback(
    (folder: string) => {
      go({
        scope: { kind: "folder", base: folder, label: folderLabel(folder), pinned: true },
        path: folder,
      });
    },
    [go],
  );

  /** 侧边栏的「新建分类 / 重命名」都走这里。 */
  const commitSideInput = useCallback(async () => {
    const input = sideInput;
    if (!input) return;
    const value = input.value.trim();
    setSideInput(null);
    if (!value) return;
    try {
      if (input.kind === "new") {
        await invoke("launcher_create_dir", { parent: input.path, name: value });
      } else {
        await invoke("launcher_rename", { path: input.path, name: value });
      }
      refreshRef.current();
    } catch (error) {
      showToastRef.current(String(error));
    }
  }, [sideInput]);

  // 拖拽用的两份最新状态：当前在哪一项、某一项点下去做什么。
  useEffect(() => {
    activeIdRef.current =
      scope.kind === "all" ? "__all__" : `${scope.pinned ? "pin" : "cat"}:${scope.base}`;
  }, [scope]);

  useEffect(() => {
    const map: Record<string, () => void> = { __all__: openAll };

    // 同一个文件夹可能既是分类又是快速访问：键得带来源，不然两边互相覆盖，
    // 拖拽时悬停到其中一个会切到另一个。
    for (const category of root.categories) {
      map[`cat:${category.path}`] = () => openCategory(category);
    }
    for (const folder of root.folders) {
      map[`pin:${folder}`] = () => openPinned(folder);
    }
    sidesRef.current = map;
  }, [root, openAll, openCategory, openPinned]);

  // ---------- 派生数据 ----------

  const crumbs = useMemo(() => {
    if (scope.kind !== "folder" || !path || !base) return [];
    const rest = path.slice(base.length).replace(/^[\\/]+/, "");
    if (!rest) return [];
    let acc = base;
    return rest
      .split(/[\\/]+/)
      .filter(Boolean)
      .map((name) => {
        acc = `${acc}\\${name}`;
        return { name, path: acc };
      });
  }, [scope, path, base]);

  // 一输入关键词就跨所有分类找：搜索框不属于某一个分类，只看眼前这一层太窄。
  // 当前目录的条目也并进来，在子文件夹 / 快速访问里搜东西时不会突然搜不到眼前这些。
  const searched = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return null;
    const pool = new Map<string, LauncherItem>();
    for (const category of root.categories) {
      for (const item of allCache[category.path] ?? []) pool.set(item.path, item);
    }
    for (const item of items) pool.set(item.path, item);
    return [...pool.values()].filter((item) => item.name.toLowerCase().includes(needle));
  }, [query, root.categories, allCache, items]);

  const filtered = searched ?? items;

  const shown = useMemo(() => sortItems(filtered, sort, usage), [filtered, sort, usage]);

  // 虚拟滚动的几何：列数、行高、总高度、当前要渲染的区间。
  const layout = useMemo(() => {
    const width = Math.max(0, viewport.width);
    const height = Math.max(0, viewport.height);
    if (view === "grid") {
      const inner = Math.max(width - GRID_PAD * 2, GRID_CELL_W);
      const cols = Math.max(1, Math.floor((inner + GRID_GAP) / (GRID_CELL_W + GRID_GAP)));
      const pitch = GRID_TILE_H + GRID_GAP;
      const rows = Math.ceil(shown.length / cols);
      const total = GRID_PAD * 2 + Math.max(0, rows * pitch - GRID_GAP);
      // 上下各多渲染两行，滚动时不会出现空白。
      const first = Math.max(0, Math.floor((viewport.scrollTop - GRID_PAD) / pitch) - 2);
      const last = Math.min(rows, Math.ceil((viewport.scrollTop + height - GRID_PAD) / pitch) + 2);
      return {
        start: first * cols,
        end: Math.min(shown.length, Math.max(last, first + 1) * cols),
        total,
        topOf: (index: number) => GRID_PAD + Math.floor(index / cols) * pitch,
        leftOf: (index: number) => GRID_PAD + (index % cols) * (GRID_CELL_W + GRID_GAP),
        itemHeight: GRID_TILE_H,
        pad: GRID_PAD,
      };
    }
    const pitch = LIST_ROW_H + LIST_GAP;
    const rows = shown.length;
    const total = LIST_PAD * 2 + Math.max(0, rows * pitch - LIST_GAP);
    const first = Math.max(0, Math.floor((viewport.scrollTop - LIST_PAD) / pitch) - 3);
    const last = Math.min(rows, Math.ceil((viewport.scrollTop + height - LIST_PAD) / pitch) + 3);
    return {
      start: first,
      end: Math.min(rows, Math.max(last, first + 1)),
      total,
      topOf: (index: number) => LIST_PAD + index * pitch,
      leftOf: () => LIST_PAD,
      itemHeight: LIST_ROW_H,
      pad: LIST_PAD,
    };
  }, [view, viewport.width, viewport.height, viewport.scrollTop, shown.length]);

  useEffect(() => {
    if (selected >= shown.length) setSelected(Math.max(shown.length - 1, -1));
  }, [shown.length, selected]);

  // 键盘移动后把选中项滚进视野。虚拟滚动下目标节点可能还没渲染，
  // 只能按几何位置算，`scrollIntoView` 靠不住。
  // 只在「选中项变了」时滚：否则用户自己往下滚，这里会把视图硬拽回选中项。
  const revealed = useRef(-1);
  useEffect(() => {
    if (selected < 0 || selected === revealed.current) return;
    revealed.current = selected;
    const element = gridRef.current;
    if (!element) return;
    const top = layout.topOf(selected);
    const bottom = top + layout.itemHeight;
    if (top - layout.pad < element.scrollTop) {
      element.scrollTop = Math.max(0, top - layout.pad);
    } else if (bottom + layout.pad > element.scrollTop + element.clientHeight) {
      element.scrollTop = bottom + layout.pad - element.clientHeight;
    }
  }, [selected, layout]);

  // 图标只取屏幕上看得见的那些，再往后多要一截：滚动的时候不用等。
  useEffect(() => {
    // 可见的只按「可见请求」去重：预取排着队的，滚到眼前要再要一次，
    // 后端会把它们从预取队挪到可见队。
    const pickVisible = (paths: string[]) =>
      paths.filter((path) => !(path in iconRef.current) && !pendingIcons.current.has(path));
    const known = (path: string) => path in iconRef.current || pendingIcons.current.has(path);
    const visible = pickVisible(shown.slice(layout.start, layout.end).map((item) => item.path));
    const ahead = shown
      .slice(layout.end, Math.min(shown.length, layout.end + ICON_LOOKAHEAD))
      .map((item) => item.path)
      .filter((path) => !known(path) && !pendingAhead.current.has(path));
    if (visible.length || ahead.length) fillIcons(visible, ahead);
  }, [shown, layout.start, layout.end, fillIcons]);

  // 换目录、换排序、切列表/网格：新内容淡入 + 轻轻上移一点。虚拟滚动的容器
  // 始终是同一个节点，所以手动摘掉再挂上这个类，动画才会重播。
  useEffect(() => {
    const element = gridRef.current;
    if (!element) return;
    element.classList.remove("view-in");
    const frame = requestAnimationFrame(() => element.classList.add("view-in"));
    return () => cancelAnimationFrame(frame);
  }, [scope, path, sort, view, loadTick]);

  // ---------- 操作 ----------

  const refreshAll = useCallback(async () => {
    await reload(true);
    if (scope.kind === "folder") {
      const target = path || scope.base;
      if (target) {
        setItems(await invoke<LauncherItem[]>("launcher_items", { path: target, withIcons: false }));
      }
    }
  }, [reload, scope, path]);

  const [warmTick, setWarmTick] = useState(0);

  const warmPreview = useCallback(async () => {
    const folders: string[] = [];
    for (const category of root.categories) folders.push(category.path);
    for (const folder of root.folders) folders.push(folder);
    const done = new Set<string>();
    const wanted: string[] = [];
    for (const folder of folders) {
      if (!folder || done.has(folder)) continue;
      done.add(folder);
      let list = allCache[folder];
      if (!list) {
        try {
          list = await invoke<LauncherItem[]>("launcher_items", {
            path: folder,
            withIcons: false,
          });
        } catch {
          continue;
        }
        const loaded = list;
        setAllCache((previous) =>
          previous[folder] ? previous : { ...previous, [folder]: loaded },
        );
      }
      // 拖过去看到的就是头一屏，按当前排序取前几十个。
      for (const item of sortItems(list, sort, usage).slice(0, PREVIEW_WARM)) {
        if (warmPaths.current.has(item.path)) continue;
        warmPaths.current.add(item.path);
        wanted.push(item.path);
        warmIcon(item.path, iconRef.current[item.path] ?? null);
      }
    }
    // 没抽过的交给后端的预取队列（排在可见区后面）。
    if (wanted.length) fillIcons([], wanted);
  }, [root, allCache, sort, usage, fillIcons, warmIcon]);

  useEffect(() => {
    // 呼出后稍等一下再备：眼前这一屏排在队首，预取本来就排后面。
    const timer = window.setTimeout(() => void warmPreview(), 400);
    return () => window.clearTimeout(timer);
  }, [warmPreview, warmTick]);

  // 使用频率：每次呼出都重新拿一遍——刚打开过的文件次数变了，排序要跟上。
  useEffect(() => {
    const load = () => {
      void invoke<Record<string, number>>("launcher_usage")
        .then(setUsage)
        .catch(() => {});
    };
    load();
    const stop = listen("launcher-shown", load);
    return () => {
      void stop.then((unlisten) => unlisten());
    };
  }, []);

  // 每次呼出都重排一次：上次之后新加进这些文件夹的东西也要备上。
  useEffect(() => {
    const stop = listen("launcher-shown", () => setWarmTick((tick) => tick + 1));
    return () => {
      void stop.then((unlisten) => unlisten());
    };
  }, []);

  useEffect(() => {
    shownRef.current = shown;
  }, [shown]);
  useEffect(() => {
    refreshRef.current = () => void refreshAll();
  }, [refreshAll]);
  useEffect(() => {
    showToastRef.current = showToast;
  }, [showToast]);
  const commitRename = useCallback(
    async (item: LauncherItem) => {
      const draft = (editing?.name ?? "").replace(/[\r\n]+/g, " ").trim();
      setEditing(null);
      requestAnimationFrame(() => inputRef.current?.focus());
      if (!draft || draft === item.name) return;
      try {
        await invoke("launcher_rename", { path: item.path, name: draft });
      } catch (error) {
        showToast(String(error));
      } finally {
        void refreshAll();
      }
    },
    [editing, refreshAll, showToast],
  );

  const cancelRename = useCallback(() => {
    skipCommit.current = true;
    setEditing(null);
    requestAnimationFrame(() => inputRef.current?.focus());
  }, []);

  const openSettings = useCallback(async (tab: "items" | "folders") => {
    await invoke("open_settings", { page: "launcher", tab });
    await invoke("dismiss_launcher");
  }, []);

  const openItem = useCallback((item: LauncherItem) => {
    void invoke("launcher_open", { path: item.path })
      .then(() => invoke("dismiss_launcher"))
      .catch((error) => showToast(String(error)));
  }, []);

  const startDrag = (paths: string[], event: React.PointerEvent) => {
    if (event.button !== 0 || paths.length === 0) return;
    const startX = event.clientX;
    const startY = event.clientY;
    let started = false;

    const stop = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", stop);
    };
    const move = (next: PointerEvent) => {
      if (started) return;
      if (Math.abs(next.clientX - startX) + Math.abs(next.clientY - startY) < 5) return;
      started = true;
      stop();
      void invoke("launcher_begin_drag", { paths }).catch((error) => showToast(String(error)));
    };

    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", stop);
  };

  const beginMarquee = (event: React.PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return;
    // 格子和按钮都不该开始框选：按钮上的点击会被指针捕获吞掉。
    // 按住 Alt 是例外：在格子/行上直接拉框（否则列表模式几乎没有空白可下手）。
    if (
      !event.altKey &&
      (event.target as HTMLElement).closest("[data-index], button, input, select, textarea")
    ) {
      return;
    }
    const grid = gridRef.current;
    if (!grid) return;
    const rect = grid.getBoundingClientRect();
    const x = event.clientX - rect.left + grid.scrollLeft;
    const y = event.clientY - rect.top + grid.scrollTop;
    marqueeRef.current = { x, y, base: event.ctrlKey ? new Set(picked) : new Set() };
    if (!event.ctrlKey) setPicked(new Set());
    setMarquee({ left: x, top: y, width: 0, height: 0 });
    grid.setPointerCapture(event.pointerId);
  };

  const moveMarquee = (event: React.PointerEvent<HTMLDivElement>) => {
    const start = marqueeRef.current;
    const grid = gridRef.current;
    if (!start || !grid) return;
    const rect = grid.getBoundingClientRect();
    const x = event.clientX - rect.left + grid.scrollLeft;
    const y = event.clientY - rect.top + grid.scrollTop;
    const box = {
      left: Math.min(start.x, x),
      top: Math.min(start.y, y),
      width: Math.abs(x - start.x),
      height: Math.abs(y - start.y),
    };
    setMarquee(box);
    const next = new Set(start.base);
    grid.querySelectorAll<HTMLElement>("[data-index]").forEach((node) => {
      const r = node.getBoundingClientRect();
      const left = r.left - rect.left + grid.scrollLeft;
      const top = r.top - rect.top + grid.scrollTop;
      const hit =
        left < box.left + box.width &&
        left + r.width > box.left &&
        top < box.top + box.height &&
        top + r.height > box.top;
      if (!hit) return;
      const item = shown[Number(node.dataset.index)];
      if (item) next.add(item.path);
    });
    setPicked(next);
  };

  const endMarquee = () => {
    if (!marqueeRef.current) return;
    marqueeRef.current = null;
    setMarquee(null);
    // 框选之后焦点会掉到 body 上，键盘快捷键就没人接了。
    requestAnimationFrame(() => inputRef.current?.focus());
  };

  const onInputKeyDown = (event: React.KeyboardEvent<HTMLInputElement>) => {
    if (fileShortcuts(event)) return;
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setSelected((current) =>
        current < 0 ? 0 : Math.min(current + 1, Math.max(shown.length - 1, 0)),
      );
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setSelected((current) => (current <= 0 ? 0 : current - 1));
    } else if (event.key === "Enter") {
      event.preventDefault();
      const item = shown[selected >= 0 ? selected : 0];
      if (!item) return;
      if (item.isDir) enterFolder(item.path);
      else openItem(item);
    } else if (event.key === "Backspace" && !query) {
      event.preventDefault();
      goUp();
    }
  };

  /** Esc 的唯一处理点：改名输入框会 stopPropagation，走到这里的一定不是改名。 */
  // 删除 / 复制 / 剪切 / 粘贴：根节点和搜索框都要能用。
  const fileShortcuts = (event: React.KeyboardEvent): boolean => {
    const target = event.target as HTMLElement;
    const renaming = target.tagName === "TEXTAREA";
    const ctrl = event.ctrlKey || event.metaKey;
    const key = event.key.toLowerCase();
    if (event.key === "Delete" && picked.size > 0) {
      event.preventDefault();
      void invoke<number>("launcher_trash", { paths: [...picked] })
        .then(() => void reload())
        .catch((error) => showToastRef.current(String(error)));
      return true;
    }
    if (ctrl && (key === "c" || key === "x") && picked.size > 0 && !renaming) {
      event.preventDefault();
      void invoke("launcher_clipboard_copy", { paths: [...picked], cut: key === "x" }).catch(
        (error) => showToastRef.current(String(error)),
      );
      return true;
    }
    // Ctrl+Z：撤销上一步（删进回收站 / 重命名 / 移动 / 新建文件夹）
    if (ctrl && key === "z" && !renaming) {
      event.preventDefault();
      void invoke<string>("launcher_undo")
        .then((message) => {
          void reload();
          showToastRef.current(message);
        })
        .catch((error) => showToastRef.current(String(error)));
      return true;
    }

    if (ctrl && key === "v" && !renaming) {
      const dir = currentDir || root.root;
      if (!dir) return false;
      event.preventDefault();
      void invoke<number>("launcher_paste", { dest: dir })
        .then(() => void reload())
        .catch((error) => showToastRef.current(String(error)));
      return true;
    }
    return false;
  };

  // Ctrl+Z 用 document 级监听兜底：焦点在搜索框/改名框时 React 处理器不一定收到。
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (!(event.ctrlKey || event.metaKey) || event.key.toLowerCase() !== "z") return;
      event.preventDefault();
      void invoke<string>("launcher_undo")
        .then((message) => {
          void reload();
          showToastRef.current(message);
        })
        .catch((error) => showToastRef.current(String(error)));
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [reload]);

  // 侧边栏是「悬停即切换」，但鼠标划过整列时会一路触发，
  // 列表跟着重排，看起来就是分类和常用文件夹来回亮、来回跳。
  // 主题跟随设置（不只是系统）：面板要跟设置窗口一致
  useEffect(() => {
    const apply = () =>
      void invoke<string>("ui_theme_preference").then((preference) => {
        const dark =
          preference === "dark" ||
          (preference === "system" &&
            window.matchMedia("(prefers-color-scheme: dark)").matches);
        document.documentElement.dataset.theme = dark ? "dark" : "light";
      });
    apply();
    const query = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => apply();
    query.addEventListener("change", onChange);
    return () => query.removeEventListener("change", onChange);
  }, []);

  // 高亮由 CSS :hover 立刻响应，这里只控制"切换内容"的时机。
  const hoverTimer = useRef<number | null>(null);
  const hoverOpen = (action: () => void) => {
    if (hoverTimer.current !== null) window.clearTimeout(hoverTimer.current);
    hoverTimer.current = window.setTimeout(() => {
      hoverTimer.current = null;
      action();
    }, 25);
  };
  const hoverCancel = () => {
    if (hoverTimer.current !== null) {
      window.clearTimeout(hoverTimer.current);
      hoverTimer.current = null;
    }
  };

  const onRootKeyDown = (event: React.KeyboardEvent) => {
    if (fileShortcuts(event)) return;
    const target = event.target as HTMLElement;
    // 改名时 Ctrl+A 应该选中名字本身，而不是全选文件。
    const renaming = target.tagName === "TEXTAREA";
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "a" && !renaming) {
      event.preventDefault();
      setPicked(new Set(shown.map((item) => item.path)));
      return;
    }
    // 删除到回收站（系统会弹确认框）
    if (event.key === "Delete" && picked.size > 0) {
      event.preventDefault();
      void invoke<number>("launcher_trash", { paths: [...picked] })
        .then(() => void reload())
        .catch((error) => showToastRef.current(String(error)));
      return;
    }

    const ctrl = event.ctrlKey || event.metaKey;
    const key = event.key.toLowerCase();
    // 复制 / 剪切：写进系统剪贴板，资源管理器也能粘
    if (ctrl && (key === "c" || key === "x") && picked.size > 0 && !renaming) {
      event.preventDefault();
      void invoke("launcher_clipboard_copy", { paths: [...picked], cut: key === "x" }).catch(
        (error) => showToastRef.current(String(error)),
      );
      return;
    }
    // 粘贴到当前目录
    if (ctrl && key === "v" && !renaming) {
      const dir = currentDir || root.root;
      if (!dir) return;
      event.preventDefault();
      void invoke<number>("launcher_paste", { dest: dir })
        .then(() => void reload())
        .catch((error) => showToastRef.current(String(error)));
      return;
    }

    if (event.key !== "Escape") return;
    event.preventDefault();
    // 先取消多选，再退层，最后才关面板。
    if (picked.size > 0) {
      setPicked(new Set());
      return;
    }
    if (scope.kind === "folder" && path && path !== base) {
      goUp();
      return;
    }
    setQuery("");
    void invoke("dismiss_launcher");
  };

  const hasRoot = root.root.trim().length > 0;
  // 同一个路径可能既是分类又是常用文件夹，id 要带来源，否则两个一起亮。
  const activeId =
    scope.kind === "all" ? "__all__" : `${scope.pinned ? "pin" : "cat"}:${scope.base}`;
  const baseLabel = scope.kind === "all" ? "全部" : scope.label;

  // 侧边栏的选中高亮是一块会滑动的形状：切换分类时从上一行滑到下一行，
  // 而不是两行各自闪一下底色。位置在绘制前算好，避免先闪在顶上。
  const sidebarRef = useRef<HTMLElement | null>(null);
  const sideHlRef = useRef<HTMLElement | null>(null);
  const sideHlPlaced = useRef(false);
  useLayoutEffect(() => {
    const rail = sidebarRef.current;
    const pill = sideHlRef.current;
    if (!rail || !pill) return;
    const active = rail.querySelector<HTMLElement>(".side-item.active");
    if (!active) {
      pill.style.opacity = "0";
      return;
    }
    pill.style.opacity = "1";
    pill.style.height = `${active.offsetHeight}px`;
    pill.style.transform = `translateY(${active.offsetTop}px)`;
    if (!sideHlPlaced.current) {
      // 第一次出现时别从顶上滑下来。
      sideHlPlaced.current = true;
      pill.classList.add("still");
      requestAnimationFrame(() => pill.classList.remove("still"));
    }
  }, [activeId, root.categories, root.folders, sideInput, viewport.width, viewport.height]);

  // 无边框窗口没有系统缩放边框，放 8 个小手柄交给系统去做缩放。
  const resizeHandles = (
    <>
      {(
        [
          ["n", "North", "ns-resize", "top:0;left:8px;right:8px;height:5px"],
          ["s", "South", "ns-resize", "bottom:0;left:8px;right:8px;height:5px"],
          ["w", "West", "ew-resize", "left:0;top:8px;bottom:8px;width:5px"],
          ["e", "East", "ew-resize", "right:0;top:8px;bottom:8px;width:5px"],
          ["nw", "NorthWest", "nwse-resize", "top:0;left:0;width:10px;height:10px"],
          ["ne", "NorthEast", "nesw-resize", "top:0;right:0;width:10px;height:10px"],
          ["sw", "SouthWest", "nesw-resize", "bottom:0;left:0;width:10px;height:10px"],
          ["se", "SouthEast", "nwse-resize", "bottom:0;right:0;width:10px;height:10px"],
        ] as const
      ).map(([key, direction, cursor, style]) => (
        <div
          key={key}
          style={{ position: "fixed", zIndex: 9999, cursor, ...cssFrom(style) }}
          onPointerDown={(event) => {
            event.preventDefault();
            event.stopPropagation();
            void getCurrentWindow().startResizeDragging(direction);
          }}
        />
      ))}
    </>
  );

  return (
    <div
      className="palette"
      ref={paletteRef}
      onKeyDown={onRootKeyDown}
      onMouseDown={() => {
        if (sideMenu) setSideMenu(null);
      }}
    >
      {resizeHandles}
      <aside
        className="sidebar"
        ref={sidebarRef}
        onContextMenu={(event) => {
          if ((event.target as HTMLElement).closest("[data-side]")) return;
          event.preventDefault();
          setSideMenu({ x: event.clientX, y: event.clientY, path: "", label: "" });
        }}
      >
        <i className="side-hl" ref={sideHlRef} aria-hidden="true" />
        {sideInput && (
          <div className="side-input-row">
            <input
              className="side-input"
              autoFocus
              value={sideInput.value}
              placeholder={sideInput.kind === "new" ? "分类名" : "新名字"}
              onChange={(event) => setSideInput({ ...sideInput, value: event.target.value })}
              onKeyDown={(event) => {
                event.stopPropagation();
                if (event.key === "Enter" || event.keyCode === 13) void commitSideInput();
                if (event.key === "Escape") setSideInput(null);
              }}
            />
            <button
              type="button"
              className="side-ok"
              title="确定"
              onClick={() => void commitSideInput()}
            >
              ✓
            </button>
          </div>
        )}
        {root.allVisible && (
          <button
            type="button"
            className={`side-item${activeId === "__all__" ? " active" : ""}`}
            data-side="__all__"
            onClick={openAll}
            onMouseEnter={() => {
              if (activeId !== "__all__") hoverOpen(openAll);
            }}
            onMouseLeave={hoverCancel}
          >
            <span>全部</span>
          </button>
        )}

        {root.categories.filter((category) => !category.hidden).map((category) => (
          <button
            type="button"
            key={category.path}
            className={`side-item${activeId === `cat:${category.path}` ? " active" : ""}`}
            data-side={`cat:${category.path}`}
            data-path={category.path}
            onClick={() => openCategory(category)}
            onContextMenu={(event) => {
              event.preventDefault();
              event.stopPropagation();
              setSideMenu({
                x: event.clientX,
                y: event.clientY,
                path: category.path,
                label: category.name,
              });
            }}
            onMouseEnter={() => {
              if (activeId !== `cat:${category.path}`) hoverOpen(() => openCategory(category));
            }}
            onMouseLeave={hoverCancel}
          >
            <span>{category.name}</span>
          </button>
        ))}
        {!hasRoot && (
          <div className="side-hint">在设置里选一个文件夹，它的子文件夹就是分类</div>
        )}

        {root.folders.length > 0 && <div className="side-sep" />}
        {root.folders.map((folder) => (
          <button
            type="button"
            key={folder}
            className={`side-item${activeId === `pin:${folder}` ? " active" : ""}`}
            data-side={`pin:${folder}`}
            data-path={folder}
            onClick={() => openPinned(folder)}
            onContextMenu={(event) => {
              event.preventDefault();
              event.stopPropagation();
              setSideMenu({ x: event.clientX, y: event.clientY, path: folder, label: folderLabel(folder) });
            }}
            onMouseEnter={() => {
              if (activeId !== `pin:${folder}`) hoverOpen(() => openPinned(folder));
            }}
            onMouseLeave={hoverCancel}
          >
            <span>{folderLabel(folder)}</span>
          </button>
        ))}
        {sideMenu && (
          <div
            className="ctx-menu"
            style={{ left: sideMenu.x, top: sideMenu.y }}
            onMouseDown={(event) => event.stopPropagation()}
            onContextMenu={(event) => event.preventDefault()}
          >
            {root.root.trim().length > 0 && (
              <button
                type="button"
                onClick={() => {
                  const menu = sideMenu;
                  setSideMenu(null);
                  setSideInput({ kind: "new", path: root.root, value: "" });
                  void menu;
                }}
              >
                新建分类
              </button>
            )}
            {sideMenu.path.length > 0 && (
              <button
                type="button"
                onClick={() => {
                  const menu = sideMenu;
                  setSideMenu(null);
                  setSideInput({ kind: "rename", path: menu.path, value: menu.label });
                }}
              >
                重命名
              </button>
            )}
          </div>
        )}
      </aside>

      <section className="content">
        <div className="toolbar">
          {/* 只有进了子文件夹才给前进/后退/上一级；停在顶层时这排是空的 */}
          {canUp && (
            <>
              <button
                type="button"
                className="tool-btn"
                disabled={backStack.length === 0}
                onClick={goBack}
                title="后退"
              >
                ←
              </button>
              <button
                type="button"
                className="tool-btn"
                disabled={forwardStack.length === 0}
                onClick={goForward}
                title="前进"
              >
                →
              </button>
              <button type="button" className="tool-btn" onClick={goUp} title="上一级">
                ↑
              </button>
            </>
          )}

          {/* 停在根目录时不显示面包屑，进了子文件夹才出现 */}
          <div className="crumbs">
            {crumbs.length > 0 && (
              <>
                <button
                  type="button"
                  data-crumb={base}
                  className={dropCrumb === base ? "crumb drop-hover" : "crumb"}
                  onClick={() => go({ scope, path: base })}
                >
                  {baseLabel}
                </button>
                {crumbs.map((crumb) => (
                  <Fragment key={crumb.path}>
                    <span className="crumb-sep">›</span>
                    <button
                      type="button"
                      data-crumb={crumb.path}
                      className={dropCrumb === crumb.path ? "crumb drop-hover" : "crumb"}
                      onClick={() => go({ scope, path: crumb.path })}
                    >
                      {crumb.name}
                    </button>
                  </Fragment>
                ))}
              </>
            )}
          </div>

          <div className="sort-wrap">
            <button
              type="button"
              className={`tool-btn${sortMenu ? " on" : ""}`}
              onClick={() => setSortMenu((open) => !open)}
              title={`排序：${SORT_LABELS[sort.key]} ${sort.dir === 1 ? "升序" : "降序"}`}
            >
              <svg width="15" height="15" viewBox="0 0 15 15" fill="none" aria-hidden="true">
                <path
                  d="M2 3.5h7M2 7.5h4.5M2 11.5h2"
                  stroke="currentColor"
                  strokeWidth="1.4"
                  strokeLinecap="round"
                />
                <path
                  d="M12 2.5v9m0 0 2-2m-2 2-2-2"
                  stroke="currentColor"
                  strokeWidth="1.4"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                />
              </svg>
            </button>
            {sortMenu && (
              <div className="sort-menu">
                {(Object.keys(SORT_LABELS) as SortKey[]).map((key) => (
                  <button
                    type="button"
                    key={key}
                    className={`sort-item${sort.key === key ? " active" : ""}`}
                    onClick={() => {
                      setSort((previous) =>
                        previous.key === key
                          ? { ...previous, dir: previous.dir === 1 ? -1 : 1 }
                          // 使用频率默认降序：最常打开的排最前面。
                          : { key, dir: key === "frequent" ? -1 : 1 },
                      );
                      setSortMenu(false);
                    }}
                  >
                    <span>{SORT_LABELS[key]}</span>
                    {sort.key === key && <span>{sort.dir === 1 ? "↑" : "↓"}</span>}
                  </button>
                ))}
              </div>
            )}
          </div>
          <button
            type="button"
            className="tool-btn"
            onClick={() => setView((current) => (current === "grid" ? "list" : "grid"))}
            title={view === "grid" ? "切换到列表" : "切换到图标"}
          >
            {view === "grid" ? (
              <svg width="15" height="15" viewBox="0 0 15 15" fill="none" aria-hidden="true">
                <path d="M2 3.5h11M2 7.5h11M2 11.5h11" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
              </svg>
            ) : (
              <svg width="15" height="15" viewBox="0 0 15 15" fill="none" aria-hidden="true">
                <rect x="2" y="2" width="4.5" height="4.5" rx="1" stroke="currentColor" strokeWidth="1.3" />
                <rect x="8.5" y="2" width="4.5" height="4.5" rx="1" stroke="currentColor" strokeWidth="1.3" />
                <rect x="2" y="8.5" width="4.5" height="4.5" rx="1" stroke="currentColor" strokeWidth="1.3" />
                <rect x="8.5" y="8.5" width="4.5" height="4.5" rx="1" stroke="currentColor" strokeWidth="1.3" />
              </svg>
            )}
          </button>
          <div className="search">
            <svg width="17" height="17" viewBox="0 0 17 17" fill="none" aria-hidden="true">
              <circle cx="7.2" cy="7.2" r="5.2" stroke="currentColor" strokeWidth="1.6" />
              <path
                d="M11.2 11.2 15 15"
                stroke="currentColor"
                strokeWidth="1.6"
                strokeLinecap="round"
              />
            </svg>
            <input
              ref={inputRef}
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              onKeyDown={onInputKeyDown}
              placeholder=""
              spellCheck={false}
              autoComplete="off"
            />
          </div>
        </div>
        <div
          className={view === "grid" ? "grid" : "list"}
          ref={gridRef}
          onScroll={(event) => {
            const element = event.currentTarget;
            setViewport((previous) =>
              previous.scrollTop === element.scrollTop
                ? previous
                : { ...previous, scrollTop: element.scrollTop },
            );
          }}
          onPointerDown={beginMarquee}
          onPointerMove={moveMarquee}
          onPointerUp={endMarquee}
          onPointerCancel={endMarquee}
          onMouseMove={(event) => {
            hoverArmed.current = true;
            const tile = (event.target as HTMLElement).closest<HTMLElement>("[data-index]");
            if (!tile) return;
            const index = Number(tile.dataset.index);
            if (Number.isFinite(index) && index !== selected) setSelected(index);
          }}
        >
          {marquee && (
            <div
              className="marquee"
              style={{
                left: marquee.left,
                top: marquee.top,
                width: marquee.width,
                height: marquee.height,
              }}
            />
          )}
          {shown.length === 0 && (
            <div className="empty">
              {!hasRoot && scope.kind === "all" ? (
                <>
                  <strong>还没有选分类的根目录</strong>
                  选一个文件夹，它的每个子文件夹就会变成左边的分类
                  <button
                    type="button"
                    className="empty-action"
                    onClick={() => void openSettings("items")}
                  >
                    去设置里选文件夹
                  </button>
                </>
              ) : searched ? (
                <>
                  <strong>所有分类里都没有</strong>
                  换个关键词试试
                </>
              ) : loading ? (
                <strong>正在读取…</strong>
              ) : (
                <>
                  <strong>这里是空的</strong>
                  往这个文件夹里放点东西
                </>
              )}
            </div>
          )}
          {/* 撑出总高度，真正渲染的只有可视范围那几十项。 */}
          <div className="virt-space" style={{ height: layout.total }} />
          {shown.slice(layout.start, layout.end).map((item, offset) => {
            const index = layout.start + offset;
            return (
            <div
              key={item.path}
              role="button"
              tabIndex={-1}
              className={`${view === "grid" ? "tile" : "row"}${
                index === selected ? " active" : ""
              }${picked.has(item.path) ? " picked" : ""}${
                dropTarget === item.path ? " drop-hover" : ""
              }`}
              title={`${item.name}${item.size ? `\n${formatSize(item.size)}` : ""}`}
              data-index={index}
              style={
                view === "grid"
                  ? {
                      position: "absolute",
                      left: layout.leftOf(index),
                      top: layout.topOf(index),
                      width: GRID_CELL_W,
                      height: GRID_TILE_H,
                    }
                  : {
                      position: "absolute",
                      left: LIST_PAD,
                      right: LIST_PAD,
                      top: layout.topOf(index),
                      height: LIST_ROW_H,
                    }
              }
              onPointerDown={(event) => {
                if (event.button !== 0) return;
                // Alt+拖动是框选，交给网格处理。
                if (event.altKey) return;
                // 点在改名框里：让它自己处理光标和选区，别抢焦点。
                if ((event.target as HTMLElement).closest(".tile-input")) return;
                const already = picked.has(item.path);
                if (event.ctrlKey || event.metaKey) {
                  setPicked((previous) => {
                    const next = new Set(previous);
                    if (next.has(item.path)) next.delete(item.path);
                    else next.add(item.path);
                    return next;
                  });
                  setSelected(index);
                  anchor.current = index;
                  return;
                }
                if (event.shiftKey) {
                  const from = anchor.current >= 0 ? anchor.current : index;
                  const [lower, upper] = from < index ? [from, index] : [index, from];
                  setPicked(new Set(shown.slice(lower, upper + 1).map((entry) => entry.path)));
                  setSelected(index);
                  return;
                }
                if (!already || picked.size <= 1) {
                  setPicked(new Set([item.path]));
                  anchor.current = index;
                }
                setSelected(index);
                requestAnimationFrame(() => inputRef.current?.focus());
                // 拖的是整批选中项；只有一个就是它自己。
                startDrag(already && picked.size > 1 ? [...picked] : [item.path], event);
              }}
              onContextMenu={(event) => {
                event.preventDefault();
                setSelected(index);
                void invoke<string | null>("launcher_context_menu", {
                  path: item.path,
                }).then((choice) => {
                  if (choice === "rename") {
                    skipCommit.current = false;
                    setEditing({ path: item.path, name: item.name });
                  } else if (choice === "deleted") {
                    void refreshAll();
                  }
                });
              }}
              onDoubleClick={() => {
                // 双击文件夹 = 进去；双击文件 = 打开。
                if (item.isDir) {
                  enterFolder(item.path);
                  return;
                }
                openItem(item);
              }}
            >
              <span className="tile-icon">
                {(() => {
                  const key = iconByPath[item.path] ?? null;
                  return key ? (
                    <img
                      className={iconReady.current.has(item.path) ? "ready" : ""}
                      src={`${ICON_BASE}${key}`}
                      alt=""
                      draggable={false}
                      decoding="async"
                      loading="lazy"
                      onLoad={(event) => {
                        // 淡入只做一次：滚回来时直接出图，不再重播。
                        if (iconReady.current.has(item.path)) return;
                        iconReady.current.add(item.path);
                        event.currentTarget.classList.add("ready");
                      }}
                      onError={() => {
                        // 缓存被清掉了：放回字形，下次可见时重新要。
                        delete iconRef.current[item.path];
                        setIconByPath({ ...iconRef.current });
                      }}
                    />
                  ) : (
                    <span className="tile-glyph">{item.isDir ? "▢" : "▤"}</span>
                  );
                })()}
              </span>
              {editing?.path === item.path ? (
                <textarea
                  className="tile-input"
                  rows={3}
                  value={editing.name}
                  ref={renameInputRef}
                  spellCheck={false}
                  onChange={(event) =>
                    setEditing({ path: item.path, name: event.target.value })
                  }
                  onKeyDown={(event) => {
                    if (event.nativeEvent.isComposing) return;
                    if (event.key === "Enter") {
                      // 文件名不能换行：回车就是确认。
                      event.preventDefault();
                      event.stopPropagation();
                      void commitRename(item);
                    } else if (event.key === "Escape") {
                      event.preventDefault();
                      event.stopPropagation();
                      cancelRename();
                    }
                  }}
                  onBlur={(event) => {
                    if (skipCommit.current) {
                      skipCommit.current = false;
                      return;
                    }
                    // 焦点只是在这块格子里移动（比如点了一下文字）就别急着提交。
                    const next = event.relatedTarget as HTMLElement | null;
                    const box = next?.closest<HTMLElement>("[data-index]");
                    if (box?.dataset.index === String(index)) {
                      return;
                    }
                    void commitRename(item);
                  }}
                  onClick={(event) => event.stopPropagation()}
                  onPointerDown={(event) => event.stopPropagation()}
                  onDoubleClick={(event) => event.stopPropagation()}
                />
              ) : (
                <span className="tile-name">{displayName(item.name)}</span>
              )}
              {view === "list" && (
                <>
                  <span className="row-meta">{item.isDir ? "" : formatSize(item.size)}</span>
                  <span className="row-meta date">{formatDate(item.modified)}</span>
                </>
              )}
            </div>
            );
          })}
        </div>

        {toast && <div className="toast">{toast}</div>}

        <div className="foot">
          {/* 一搜索就跨所有分类找，这里跟着说清楚范围。 */}
          <span>{searched ? "所有分类" : baseLabel}</span>
          <span>·</span>
          <span>{shown.length} 项</span>
          <span>·</span>
          <span>双击打开 · 拖动拖出 · 空白处或 Alt+拖动框选 · Ctrl+A 全选</span>
          <span>·</span>
          <span>
            <kbd>Esc</kbd> {canUp ? "上一层" : "关闭"}
          </span>
        </div>
      </section>

    </div>
  );
}

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <Launcher />
  </React.StrictMode>,
);
