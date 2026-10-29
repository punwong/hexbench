import "./shell";

import React, { Fragment, useCallback, useEffect, useMemo, useRef, useState } from "react";
import ReactDOM from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

type LauncherItem = {
  name: string;
  path: string;
  isDir: boolean;
  size: number;
  modified: number;
  icon: string | null;
};

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
  rememberLocation: boolean;
  lastKind: string;
  lastBase: string;
  lastLabel: string;
  lastPath: string;
};

type SortKey = "name" | "modified" | "size" | "type";

/// 和资源管理器一致的排序：数字按数值比（`图2 < 图10`）。
/// 资源管理器用的是 Shell 的 StrCmpLogicalW，`numeric` 就是它的等价物。
const collator = new Intl.Collator("zh-CN", { numeric: true, sensitivity: "base" });

const SORT_LABELS: Record<SortKey, string> = {
  name: "名称",
  modified: "修改时间",
  size: "大小",
  type: "类型",
};

/** 面板现在浏览的是哪一片区域。 */
type Scope =
  | { kind: "all" }
  /** 一棵真实的目录树：某个分类，或者一个常用文件夹。 */
  | { kind: "folder"; base: string; label: string };

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
    rememberLocation: false,
    lastKind: "",
    lastBase: "",
    lastLabel: "",
    lastPath: "",
  });
  const [allCache, setAllCache] = useState<Record<string, LauncherItem[]>>({});
  const [items, setItems] = useState<LauncherItem[]>([]);
  const [loading, setLoading] = useState(false);
  // 图标单独存：先出名字，图标随后分批补，切目录时不会串。
  const [iconByPath, setIconByPath] = useState<Record<string, string | null>>({});
  const iconRef = useRef<Record<string, string | null>>({});
  const viewToken = useRef(0);

  const fillIcons = useCallback(async (paths: string[]) => {
    const token = viewToken.current;
    const missing = paths.filter((path) => !(path in iconRef.current));
    for (let index = 0; index < missing.length; index += 24) {
      if (token !== viewToken.current) return; // 已经切到别的目录了
      const chunk = missing.slice(index, index + 24);
      const filled = await invoke<Array<[string, string | null]>>("launcher_icons", {
        paths: chunk,
      });
      if (token !== viewToken.current) return;
      const next = { ...iconRef.current };
      for (const [path, icon] of filled) next[path] = icon;
      iconRef.current = next;
      setIconByPath(next);
    }
  }, []);

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
  // 拖拽结束后逼加载器重跑一次（拖拽期间可能停在没缓存的目录上）。
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
  const [sortMenu, setSortMenu] = useState(false);
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
  const sidesRef = useRef<Record<string, () => void>>({});
  const base = scope.kind === "folder" ? scope.base : "";
  const currentDir = scope.kind === "folder" ? path || scope.base : root.root;

  const reload = useCallback(async (): Promise<LauncherRoot | null> => {
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
    setAllCache((previous) => ({ ...previous, ...loaded }));
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
        if (listing.view === "list" || listing.view === "grid") setView(listing.view);
        const keys: SortKey[] = ["name", "modified", "size", "type"];
        const key = keys.find((entry) => entry === listing.sortKey);
        if (key) setSort({ key, dir: listing.sortDir === -1 ? -1 : 1 });
      }
      if (listing?.rememberLocation && listing.lastKind === "folder" && known) {
        setScope({ kind: "folder", base, label: listing.lastLabel || "文件夹" });
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
    const folderAt = (x: number, y: number) => {
      const ratio = window.devicePixelRatio || 1;
      const node = document.elementFromPoint(x / ratio, y / ratio);
      const row = node?.closest<HTMLElement>("[data-index]");
      if (!row) return null;
      const item = shownRef.current[Number(row.dataset.index)];
      return item && item.isDir ? item : null;
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
    /** 光标下面是侧边栏的哪一项（返回 data-side 的值）。 */
    const sideAtClient = (x: number, y: number): string | null => {
      const node = document.elementFromPoint(x, y);
      const side = node?.closest<HTMLElement>("[data-side]");
      return side?.dataset.side ?? null;
    };
    void listen<{ x: number; y: number }>("launcher-drag-pos", (event) => {
      dragActiveRef.current = true;
      const point = toClient(event.payload.x, event.payload.y);
      setDropTarget(folderAtClient(point.x, point.y)?.path ?? null);
      // 拖着东西悬到分类上：切过去，这样才落得进去。
      // 拖拽期间浏览器不发 mouseenter，只能靠后端轮询来的坐标自己判断。
      const side = sideAtClient(point.x, point.y);
      if (side && side !== activeIdRef.current) {
        const switchTo = sidesRef.current[side];
        if (switchTo) switchTo();
      }
    }).then((fn) => disposers.push(fn));
    void listen<{ x: number; y: number; paths: string[] }>("launcher-drag-drop", (event) => {
      dragActiveRef.current = false;
      setLoadTick((tick) => tick + 1);
      setDropTarget(null);
      const point = toClient(event.payload.x, event.payload.y);
      const folder = folderAtClient(point.x, point.y);
      const side = sideAtClient(point.x, point.y);
      const dest = (side && side !== "__all__" ? side : null) ?? folder?.path ?? currentDirRef.current;
      if (!dest) return;
      void invoke("launcher_move_into", { dest, paths: event.payload.paths })
        .then(() => refreshRef.current())
        .catch((error) => showToastRef.current(String(error)));
    }).then((fn) => disposers.push(fn));
    // 后端收起面板（切到别的应用）时，把网页状态一起清掉。
    void listen("launcher-drag-leave", () => {
      dragActiveRef.current = false;
      setLoadTick((tick) => tick + 1);
      setDropTarget(null);
    }).then((fn) => disposers.push(fn));
    void listen<{ x: number; y: number; paths: string[] }>("launcher-dropped", (event) => {
      dragActiveRef.current = false;
      setLoadTick((tick) => tick + 1);
      setDropTarget(null);
      const item = folderAt(event.payload.x, event.payload.y);
      const side = sideAtClient(event.payload.x, event.payload.y);
      const dest = (side && side !== "__all__" ? side : null) ?? item?.path ?? currentDirRef.current;
      if (!dest) return;
      void invoke("launcher_move_into", { dest, paths: event.payload.paths })
        .then(() => refreshRef.current())
        .catch((error) => showToastRef.current(String(error)));
    }).then((fn) => disposers.push(fn));
    return () => disposers.forEach((fn) => fn());
  }, []);

  // 视图 / 排序变化也记下来。
  useEffect(() => {
    const timer = window.setTimeout(() => {
      void invoke("launcher_save_view", {
        view,
        sortKey: sort.key,
        sortDir: sort.dir,
      }).catch(() => {});
    }, 400);
    return () => window.clearTimeout(timer);
  }, [view, sort.key, sort.dir]);

  // 位置变化就记下来（防抖，避免每次点击都写盘）。
  useEffect(() => {
    if (!root.rememberLocation) return;
    const timer = window.setTimeout(() => {
      void invoke("launcher_save_location", {
        kind: scope.kind,
        base: scope.kind === "folder" ? scope.base : "",
        label: scope.kind === "folder" ? scope.label : "",
        path: scope.kind === "folder" ? path : "",
      });
    }, 400);
    return () => window.clearTimeout(timer);
  }, [scope, path, root.rememberLocation]);

  // 读当前这一层：先出名字，图标后台补。
  useEffect(() => {
    let alive = true;
    viewToken.current += 1;
    const run = async () => {
      if (scope.kind === "all" && !path) {
        const all = root.categories.flatMap((category) => allCache[category.path] ?? []);
        if (!alive) return;
        setItems(all);
        setLoading(false);
        void fillIcons(all.map((item) => item.path));
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
        void fillIcons(cached.map((item) => item.path));
        return;
      }
      // 没缓存又正在拖：保持原样，别转圈，松手后再补。
      if (dragActiveRef.current) {
        setLoading(false);
        return;
      }
      // 先清空：否则切目录时上一个目录的内容会留在屏幕上，搜索就会「搜到别处」。
      setItems([]);
      setLoading(true);
      const list = await invoke<LauncherItem[]>("launcher_items", {
        path: target,
        withIcons: false,
      });
      if (!alive) return;
      setItems(list);
      setLoading(false);
      void fillIcons(list.map((item) => item.path));
    };
    void run();
    return () => {
      alive = false;
    };
  }, [scope, path, root.categories, allCache, fillIcons, loadTick]);

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
    void listen("launcher-changed", () => void reload()).then((fn) => disposers.push(fn));
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

  const go = useCallback(
    (next: Snapshot) => {
      // 切回文件视图时把内嵌网页关掉。
      setBackStack((previous) => [...previous, { scope, path }]);
      setForwardStack([]);
      setScope(next.scope);
      setPath(next.path);
      resetView();
    },
    [scope, path, resetView],
  );

  const goBack = useCallback(() => {
    const target = backStack[backStack.length - 1];
    if (!target) return;
    setBackStack((previous) => previous.slice(0, -1));
    setForwardStack((forward) => [...forward, { scope, path }]);
    setScope(target.scope);
    setPath(target.path);
    resetView();
  }, [backStack, scope, path, resetView]);

  const goForward = useCallback(() => {
    const target = forwardStack[forwardStack.length - 1];
    if (!target) return;
    setForwardStack((previous) => previous.slice(0, -1));
    setBackStack((back) => [...back, { scope, path }]);
    setScope(target.scope);
    setPath(target.path);
    resetView();
  }, [forwardStack, scope, path, resetView]);

  const canUp = scope.kind === "folder" && !!path && path !== base;

  const goUp = useCallback(() => {
    if (scope.kind !== "folder" || !path || path === scope.base) return;
    go({ scope, path: parentOf(path) });
  }, [go, scope, path]);

  const enterFolder = useCallback(
    (folderPath: string) => {
      if (scope.kind === "all") {
        // 「全部」是多个分类的合集：进某个文件夹等于切到它所属的分类。
        const owner = root.categories.find((category) => isInside(folderPath, category.path));
        if (owner) {
          go({ scope: { kind: "folder", base: owner.path, label: owner.name }, path: folderPath });
          return;
        }
      }
      go({ scope, path: folderPath });
    },
    [go, scope, root.categories],
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
        scope: { kind: "folder", base: folder, label: folderLabel(folder) },
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
    activeIdRef.current = scope.kind === "all" ? "__all__" : scope.base;
  }, [scope]);

  useEffect(() => {
    const map: Record<string, () => void> = { __all__: openAll };

    for (const category of root.categories) {
      map[category.path] = () => openCategory(category);
    }
    for (const folder of root.folders) {
      map[folder] = () => openPinned(folder);
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

  const filtered = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return items;
    return items.filter((item) => item.name.toLowerCase().includes(needle));
  }, [items, query]);

  const shown = useMemo(() => {
    const list = [...filtered];
    list.sort((a, b) => {
      // 文件夹永远在前，和资源管理器一致。
      if (a.isDir !== b.isDir) return a.isDir ? -1 : 1;
      let result = 0;
      switch (sort.key) {
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
      return result * sort.dir;
    });
    return list;
  }, [filtered, sort]);

  useEffect(() => {
    if (selected >= shown.length) setSelected(Math.max(shown.length - 1, -1));
  }, [shown.length, selected]);

  useEffect(() => {
    if (selected < 0) return;
    const grid = gridRef.current;
    const active = grid?.querySelector<HTMLElement>("[data-index].active");
    active?.scrollIntoView({ block: "nearest" });
  }, [selected, shown]);

  // ---------- 操作 ----------

  const refreshAll = useCallback(async () => {
    await reload();
    if (scope.kind === "folder") {
      const target = path || scope.base;
      if (target) {
        setItems(await invoke<LauncherItem[]>("launcher_items", { path: target, withIcons: false }));
      }
    }
  }, [reload, scope, path]);

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

  const createFolder = useCallback(async () => {
    if (!currentDir) return;
    const created = await invoke<string>("launcher_new_folder", { parent: currentDir });
    await refreshAll();
    setEditing({ path: created, name: "新建文件夹" });
  }, [currentDir, refreshAll]);

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
  const activeId = scope.kind === "all" ? "__all__" : scope.base;
  const baseLabel = scope.kind === "all" ? "全部" : scope.label;

  return (
    <div
      className="palette"
      onKeyDown={onRootKeyDown}
      onMouseDown={() => {
        if (sideMenu) setSideMenu(null);
      }}
    >
      <aside
        className="sidebar"
        onContextMenu={(event) => {
          if ((event.target as HTMLElement).closest("[data-side]")) return;
          event.preventDefault();
          setSideMenu({ x: event.clientX, y: event.clientY, path: "", label: "" });
        }}
      >
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
              if (activeId !== "__all__") openAll();
            }}
          >
            <span>全部</span>
          </button>
        )}

        {root.categories.filter((category) => !category.hidden).map((category) => (
          <button
            type="button"
            key={category.path}
            className={`side-item${activeId === category.path ? " active" : ""}`}
            data-side={category.path}
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
              if (activeId !== category.path) openCategory(category);
            }}
            title={category.path}
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
            className={`side-item${activeId === folder ? " active" : ""}`}
            data-side={folder}
            onClick={() => openPinned(folder)}
            onContextMenu={(event) => {
              event.preventDefault();
              event.stopPropagation();
              setSideMenu({ x: event.clientX, y: event.clientY, path: folder, label: folderLabel(folder) });
            }}
            onMouseEnter={() => {
              if (activeId !== folder) openPinned(folder);
            }}
            title={folder}
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
                  className="crumb"
                  onClick={() => go({ scope, path: base })}
                >
                  {baseLabel}
                </button>
                {crumbs.map((crumb) => (
                  <Fragment key={crumb.path}>
                    <span className="crumb-sep">›</span>
                    <button
                      type="button"
                      className="crumb"
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
                          : { key, dir: 1 },
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
          <button
            type="button"
            className="tool-btn wide"
            disabled={!currentDir}
            onClick={() => void createFolder()}
            title="在当前目录新建文件夹"
          >
            新建文件夹
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
                  <strong>还没有选启动器文件夹</strong>
                  选一个文件夹，它的每个子文件夹就会变成左边的分类
                  <button
                    type="button"
                    className="empty-action"
                    onClick={() => void openSettings("items")}
                  >
                    去设置里选文件夹
                  </button>
                </>
              ) : query ? (
                <>
                  <strong>没有匹配的条目</strong>
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
          {shown.map((item, index) => (
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
                  const icon = item.icon ?? iconByPath[item.path] ?? null;
                  return icon ? (
                    <img src={`data:image/png;base64,${icon}`} alt="" draggable={false} />
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
          ))}
        </div>

        {toast && <div className="toast">{toast}</div>}

        <div className="foot">
          <span>{baseLabel}</span>
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
