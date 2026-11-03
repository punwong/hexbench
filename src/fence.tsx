import "./shell";

import React, {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import ReactDOM from "react-dom/client";
import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  ICON_BASE,
  SORT_LABELS,
  displayName,
  folderLabel,
  sortItems,
  stepZoom,
  type LauncherItem,
  type LauncherPage,
  type SortKey,
} from "./fileview";
import { watchTheme } from "./theme";
import { GESTURE_DEFAULT, useMouseGestures, type GestureSettings } from "./gesture";
import type { FenceSettings } from "./types";

/// 后端 `fence_state` 的返回。
type FenceState = {
  fence: FenceSettings;
  /** 展开状态的物理外框 `[x, y, w, h]`（调试 / 画参考线用） */
  rect: [number, number, number, number];
  scale: number;
  /** 和快捷面板共用的右键手势设置 */
  gestures?: GestureSettings;
};

/// 后端 `fence_appearance` 的返回（推送的 `fence-appearance` 只带前两项）。
type FenceAppearance = {
  /** 卡片不透明度百分比 */
  opacity: number;
  /** 设置里开没开磨砂 */
  frosted: boolean;
  /** 这台机器上材质是不是真的上了 */
  backdrop?: boolean;
};

/// 一屏格子最多显示多少个：几千个条目也只渲染几十个节点。
/// 尺寸是 100% 缩放下的逻辑像素，其余按 zoom 乘。
const CELL_W = 86;
const CELL_H = 88;
const CELL_GAP = 12;
const CELL_PAD = 10;

function normalizePath(path: string): string {
  return path.replace(/\//g, "\\").replace(/\\+$/, "").toLowerCase();
}

function isInside(child: string, parent: string): boolean {
  if (!child || !parent) return false;
  const a = normalizePath(child);
  const b = normalizePath(parent);
  return a === b || a.startsWith(`${b}\\`);
}

function parentOf(path: string): string {
  const trimmed = path.replace(/\\+$/, "");
  const cut = trimmed.lastIndexOf("\\");
  if (cut <= 2) return trimmed;
  return trimmed.slice(0, cut);
}

function Fence() {
  const id = useMemo(() => new URLSearchParams(window.location.search).get("id") ?? "", []);
  // `listen()` 不指定 target 时等于「任何目标」：后端 `emit_to` 给某个格子的事件，
  // 所有格子窗口都会收到。拖进 A 盒时 B、C 盒会跟着把同一个文件各搬一份。
  // 所以每个监听都钉死在这个窗口自己的 label 上。
  const label = useMemo(() => `fence-${id}`, [id]);
  const [state, setState] = useState<FenceState | null>(null);
  const [items, setItems] = useState<LauncherItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [path, setPath] = useState("");
  const [zoom, setZoom] = useState(100);
  const [size, setSize] = useState({ w: 0, h: 0 });
  const [scrollTop, setScrollTop] = useState(0);
  const [icons, setIcons] = useState<Record<string, string | null>>({});
  const [dropHover, setDropHover] = useState<string | null>(null);
  const [picked, setPicked] = useState<string | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const [renaming, setRenaming] = useState<{ path: string; name: string } | null>(null);
  const [renamingFence, setRenamingFence] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const [gestures, setGestures] = useState<GestureSettings>(GESTURE_DEFAULT);
  // 手势用的浏览历史：真栈放 ref（同一帧里连划两次也读得到最新值），
  // state 只留层数，给方向提示判断「还能不能退 / 进」。
  const backStack = useRef<string[]>([]);
  const forwardStack = useRef<string[]>([]);
  const [historyCounts, setHistoryCounts] = useState({ back: 0, forward: 0 });

  const bodyRef = useRef<HTMLDivElement>(null);
  const renameRef = useRef<HTMLInputElement>(null);
  const titleRef = useRef<HTMLInputElement>(null);
  const loadSeq = useRef(0);
  const currentPath = useRef("");
  const rootPath = useRef("");
  const iconRequested = useRef<Set<string>>(new Set());
  const dropTimer = useRef(0);
  const toastTimer = useRef(0);
  const zoomTimer = useRef(0);
  const refreshTimer = useRef(0);

  const showToast = useCallback((message: string) => {
    setToast(message);
    window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(null), 2600);
  }, []);

  // ---------- 数据 ----------

  const loadDir = useCallback(async (dir: string) => {
    const seq = ++loadSeq.current;
    const collected: LauncherItem[] = [];
    iconRequested.current = new Set();
    setLoading(true);
    setLoadError(null);
    setItems([]);
    setPicked(null);
    setScrollTop(0);
    const channel = new Channel<LauncherPage>();
    channel.onmessage = (page) => {
      if (seq !== loadSeq.current) return;
      collected.push(...page.items);
      setItems([...collected]);
      if (page.done) {
        setLoading(false);
        // 空列表本身不说明问题：路径被删掉时也是空列表。
        if (collected.length === 0) {
          void invoke<boolean>("fence_path_exists", { path: dir })
            .then((exists) => {
              if (seq === loadSeq.current && !exists) setLoadError("文件夹不在了");
            })
            .catch(() => undefined);
        }
      }
    };
    try {
      await invoke("launcher_items_stream", { path: dir, onPage: channel });
      if (seq === loadSeq.current) setLoading(false);
    } catch (error) {
      if (seq === loadSeq.current) {
        setLoadError(String(error));
        setLoading(false);
      }
    }
  }, []);

  const applyState = useCallback(
    async (reload: boolean) => {
      let next: FenceState | null = null;
      try {
        next = await invoke<FenceState | null>("fence_state", { id });
      } catch {
        return;
      }
      if (!next) return;
      setState(next);
      setZoom(next.fence.zoom);
      if (next.gestures) setGestures(next.gestures);
      // 换了绑定的文件夹：旧历史里的路径可能已经在新根目录外面，整段丢掉。
      if (rootPath.current !== next.fence.path) {
        rootPath.current = next.fence.path;
        backStack.current = [];
        forwardStack.current = [];
        setHistoryCounts({ back: 0, forward: 0 });
      }
      const current = currentPath.current;
      const target = current && isInside(current, next.fence.path) ? current : next.fence.path;
      if (!target) {
        setLoadError("没有绑定文件夹");
        setLoading(false);
        return;
      }
      if (target !== current) {
        currentPath.current = target;
        setPath(target);
        if (reload) await loadDir(target);
      } else if (reload) {
        await loadDir(target);
      }
    },
    [id, loadDir],
  );

  const scheduleRefresh = useCallback(() => {
    window.clearTimeout(refreshTimer.current);
    refreshTimer.current = window.setTimeout(() => {
      void applyState(true);
    }, 220);
  }, [applyState]);

  // 主题跟随设置（和面板 / 设置窗口同一套）。
  useEffect(() => watchTheme(), []);

  // 设置页改了手势（开关 / 方向 / 距离 / 轨迹）：后台推给每个格子，不重开就生效。
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void listen<GestureSettings>("gesture-settings", (event) => setGestures(event.payload), {
      target: label,
    }).then((dispose) => (unlisten = dispose));
    return () => unlisten?.();
  }, [label]);

  // 卡片外观（不透明度 / 磨砂）：窗口等第一帧之后才显示，这里来得及在画出来
  // 之前定下来。设置页改完会推 `fence-appearance`，不用重开格子。
  useEffect(() => {
    const paint = (appearance: FenceAppearance) => {
      const root = document.documentElement;
      // 有磨砂才是「半透明+模糊」；磨砂打开但这台机器上不了材质时不留透明，
      // 否则透出来的是 WebView 的白底，卡片会发灰。关掉磨砂则照常透。
      const seeThrough = appearance.frosted ? appearance.backdrop : true;
      const alpha = seeThrough
        ? Math.min(1, Math.max(0.3, appearance.opacity / 100))
        : 0.97;
      root.style.setProperty("--card-alpha", alpha.toFixed(3));
      root.style.setProperty("--header-alpha", Math.max(0.3, alpha - 0.12).toFixed(3));
      if (appearance.backdrop) {
        root.dataset.backdrop = "acrylic";
      } else {
        delete root.dataset.backdrop;
      }
    };
    void invoke<FenceAppearance>("fence_appearance").then(paint);
    let unlisten: (() => void) | undefined;
    void listen<FenceAppearance>("fence-appearance", (event) => paint(event.payload), {
      target: label,
    }).then((dispose) => (unlisten = dispose));
    return () => unlisten?.();
  }, [label]);

  // 初次挂载：取配置 + 列目录。
  useEffect(() => {
    void applyState(true);
  }, [applyState]);

  // 目录变化时告诉后端：watcher 也要盯这一层。
  useEffect(() => {
    if (!id || !path) return;
    void invoke("fence_watch_path", { id, path }).catch(() => undefined);
  }, [id, path]);
  useEffect(
    () => () => {
      if (id) void invoke("fence_watch_path", { id, path: "" }).catch(() => undefined);
    },
    [id],
  );

  // 文件变化 / 设置变化 / 右键菜单里的操作都要重新列一遍。
  useEffect(() => {
    const disposers: Array<() => void> = [];
    const add = (event: string) =>
      void listen(event, () => scheduleRefresh(), { target: label }).then((unlisten) =>
        disposers.push(unlisten),
      );
    add("fence-changed");
    add("fence-refresh");
    add("config-changed");
    add("launcher-changed");
    return () => disposers.forEach((dispose) => dispose());
  }, [scheduleRefresh, label]);

  // 规则盒自动收完东西：格子里弹一句，不然文件「自己动了」会显得莫名其妙。
  useEffect(() => {
    let dispose: (() => void) | undefined;
    void listen<{ moved: number }>(
      "fence-rules",
      (event) => {
        if (event.payload.moved > 0) showToast(`已收进 ${event.payload.moved} 项`);
      },
      { target: label },
    ).then((unlisten) => {
      dispose = unlisten;
    });
    return () => dispose?.();
  }, [showToast, label]);

  // 拖进格子：后端把光标换算成客户区物理像素。
  useEffect(() => {
    const disposers: Array<() => void> = [];
    const targetAt = (x: number, y: number) => {
      const dpr = window.devicePixelRatio || 1;
      const element = document.elementFromPoint(x / dpr, y / dpr);
      // 拖到面包屑的某一段上：松手就移进那一层，和快捷面板一样。
      const crumb = element?.closest(".crumb") as HTMLElement | null;
      if (crumb?.dataset.crumb) return crumb.dataset.crumb;
      const tile = element?.closest(".tile") as HTMLElement | null;
      if (tile?.dataset.dir === "1" && tile.dataset.path) return tile.dataset.path;
      return currentPath.current;
    };
    const position = (x: number, y: number) => {
      const dest = targetAt(x, y);
      setDropHover(dest && dest !== currentPath.current ? dest : null);
      window.clearTimeout(dropTimer.current);
      dropTimer.current = window.setTimeout(() => setDropHover(null), 700);
    };
    void listen<{ x: number; y: number }>(
      "fence-drag-over",
      (event) => {
        position(event.payload.x, event.payload.y);
      },
      { target: label },
    ).then((unlisten) => disposers.push(unlisten));
    void listen(
      "fence-drag-leave",
      () => {
        window.clearTimeout(dropTimer.current);
        setDropHover(null);
      },
      { target: label },
    ).then((unlisten) => disposers.push(unlisten));
    const dropInto = (x: number, y: number, paths: string[]) => {
      window.clearTimeout(dropTimer.current);
      setDropHover(null);
      const dest = targetAt(x, y) || currentPath.current;
      if (!dest || !paths.length) return;
      void invoke<number>("launcher_move_into", { dest, paths })
        .then((count) => {
          showToast(count > 0 ? `已移入 ${count} 项` : "没有移动");
          scheduleRefresh();
        })
        .catch((error) => showToast(String(error)));
    };
    void listen<{ x: number; y: number; paths: string[] }>(
      "fence-dropped",
      (event) => {
        dropInto(event.payload.x, event.payload.y, event.payload.paths);
      },
      { target: label },
    ).then((unlisten) => disposers.push(unlisten));
    // 从这一格自己拖出来又松手在本窗口里：OLE 落点会跳过发起拖拽的窗口，
    // 收不到 `fence-dropped`，只能靠 `run_file_drag` 松手时补发的这组事件。
    void listen<{ x: number; y: number }>(
      "fence-drag-pos",
      (event) => {
        position(event.payload.x, event.payload.y);
      },
      { target: label },
    ).then((unlisten) => disposers.push(unlisten));
    void listen<{ x: number; y: number; paths: string[] }>(
      "fence-drag-drop",
      (event) => {
        dropInto(event.payload.x, event.payload.y, event.payload.paths);
      },
      { target: label },
    ).then((unlisten) => disposers.push(unlisten));
    return () => {
      window.clearTimeout(dropTimer.current);
      disposers.forEach((dispose) => dispose());
    };
  }, [scheduleRefresh, showToast, label]);

  // 图标：缓存命中的立刻给，没命中的后端抽完一张发一条 `launcher-icon`。
  useEffect(() => {
    let dispose: (() => void) | undefined;
    void listen<{ path: string; key: string | null }>("launcher-icon", (event) => {
      const { path: iconPath, key } = event.payload;
      setIcons((previous) =>
        previous[iconPath] === key ? previous : { ...previous, [iconPath]: key },
      );
    }).then((unlisten) => {
      dispose = unlisten;
    });
    return () => dispose?.();
  }, []);

  // 内容区尺寸（虚拟滚动按它算行列）。
  useEffect(() => {
    const element = bodyRef.current;
    if (!element) return;
    const update = () => setSize({ w: element.clientWidth, h: element.clientHeight });
    update();
    const observer = new ResizeObserver(update);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  // Alt+滚轮缩放：光标锚定由布局自然保证（虚拟滚动按滚动位置算）。
  const zoomRef = useRef(zoom);
  zoomRef.current = zoom;
  useEffect(() => {
    const onWheel = (event: WheelEvent) => {
      if (!event.altKey) return;
      event.preventDefault();
      const next = stepZoom(zoomRef.current, event.deltaY < 0 ? 1 : -1);
      zoomRef.current = next;
      setZoom(next);
      window.clearTimeout(zoomTimer.current);
      zoomTimer.current = window.setTimeout(() => {
        void invoke("fence_update", { id, zoom: next }).catch(() => undefined);
      }, 400);
    };
    window.addEventListener("wheel", onWheel, { passive: false });
    return () => window.removeEventListener("wheel", onWheel);
  }, [id]);

  // ---------- 派生数据 ----------

  const sorted = useMemo(() => {
    const key = (state?.fence.sortKey as SortKey) || "name";
    const dir = state?.fence.sortDir === -1 ? -1 : 1;
    return sortItems(items, { key, dir });
  }, [items, state?.fence.sortKey, state?.fence.sortDir]);

  const z = zoom / 100;
  const cellW = Math.max(44, Math.round(CELL_W * z));
  const cellH = Math.max(44, Math.round(CELL_H * z));
  const gap = Math.max(4, Math.round(CELL_GAP * z));
  const pad = Math.max(4, Math.round(CELL_PAD * z));
  const rowH = cellH + gap;
  const cols = Math.max(1, Math.floor((size.w - pad * 2 + gap) / (cellW + gap)));
  const totalRows = Math.ceil(sorted.length / cols);
  const firstRow = Math.max(0, Math.floor((scrollTop - pad) / rowH) - 1);
  const lastRow = Math.min(
    totalRows,
    firstRow + Math.ceil((size.h || 0) / rowH) + 2,
  );
  const slice = useMemo(
    () => sorted.slice(firstRow * cols, lastRow * cols),
    [sorted, firstRow, cols, lastRow],
  );
  const sliceKey = useMemo(() => slice.map((item) => item.path).join("\n"), [slice]);

  useEffect(() => {
    if (!slice.length) return;
    const need: string[] = [];
    const ahead: string[] = [];
    for (const item of slice) {
      if (!iconRequested.current.has(item.path)) need.push(item.path);
    }
    const lookahead = sorted.slice(
      lastRow * cols,
      lastRow * cols + cols * 3,
    );
    for (const item of lookahead) {
      if (!iconRequested.current.has(item.path) && !need.includes(item.path)) {
        ahead.push(item.path);
        iconRequested.current.add(item.path);
      }
    }
    for (const path of need) iconRequested.current.add(path);
    if (!need.length && !ahead.length) return;
    void invoke<[string, string | null][]>("launcher_icons", { paths: need, ahead })
      .then((entries) => {
        if (!entries.length) return;
        setIcons((previous) => {
          const next = { ...previous };
          for (const [path, key] of entries) next[path] = key;
          return next;
        });
      })
      .catch(() => undefined);
  }, [sliceKey, cols, lastRow, sorted]);

  // ---------- 操作 ----------

  // 收纳盒：幕后目录是应用自己的，不能换文件夹；带规则时可以「立即整理」。
  const isBucket = state?.fence.kind === "bucket";
  const ruleCount = state?.fence.rules?.length ?? 0;

  const runRules = useCallback(() => {
    setMenu(null);
    void invoke<{ matched: number; moved: number }>("fence_rule_run", { id })
      .then((result) =>
        showToast(
          result.moved > 0
            ? `已整理 ${result.moved} 项`
            : result.matched > 0
              ? "没有移动成功"
              : "没有要整理的文件",
        ),
      )
      .catch((error) => showToast(String(error)));
  }, [id, showToast]);

  const root = state?.fence.path ?? "";
  const atRoot = !root || normalizePath(path) === normalizePath(root);
  const title = state
    ? atRoot
      ? state.fence.title || folderLabel(root)
      : folderLabel(path)
    : "";
  const baseLabel = state ? state.fence.title || folderLabel(root) : "";

  /// 面包屑：和快捷面板的文件区一致——停在根目录只显示标题，进了子文件夹才铺开路径，
  /// 每一段都能点回去。
  const crumbs = useMemo(() => {
    if (!root || !path || atRoot) return [];
    const base = root.replace(/[\\/]+$/, "");
    const full = path.replace(/[\\/]+$/, "");
    // 已经不在绑定文件夹里（换绑过程中 / 路径失效）就不画，免得切出半截假路径。
    if (!full.toLowerCase().startsWith(`${base.toLowerCase()}\\`)) return [];
    let acc = base;
    return full
      .slice(base.length + 1)
      .split(/[\\/]+/)
      .filter(Boolean)
      .map((name) => {
        acc = `${acc}\\${name}`;
        return { name, path: acc };
      });
  }, [atRoot, path, root]);

  /// 进入某一层：标题条跟着换，列表立刻重读（不等 watcher）。
  /// `record` 为真时记进历史，右键手势的「后退 / 前进」才有得走。
  const navigate = useCallback(
    (target: string, record = true) => {
      if (!target) return;
      const current = currentPath.current;
      if (record && current && normalizePath(current) !== normalizePath(target)) {
        backStack.current.push(current);
        // 只留最近 50 层：老目录可能已经不在了，栈也别无限涨。
        if (backStack.current.length > 50) backStack.current.shift();
        forwardStack.current = [];
        setHistoryCounts({ back: backStack.current.length, forward: 0 });
      }
      currentPath.current = target;
      setPath(target);
      void loadDir(target);
    },
    [loadDir],
  );

  const goBack = useCallback(() => {
    const target = backStack.current.pop();
    if (!target) return;
    forwardStack.current.push(currentPath.current);
    setHistoryCounts({
      back: backStack.current.length,
      forward: forwardStack.current.length,
    });
    navigate(target, false);
  }, [navigate]);

  const goForward = useCallback(() => {
    const target = forwardStack.current.pop();
    if (!target) return;
    backStack.current.push(currentPath.current);
    setHistoryCounts({
      back: backStack.current.length,
      forward: forwardStack.current.length,
    });
    navigate(target, false);
  }, [navigate]);

  // 右键手势：和快捷面板同一套设置，左右划在格子自己的目录历史里走。
  const gesture = useMouseGestures({
    settings: gestures,
    backCount: historyCounts.back,
    forwardCount: historyCounts.forward,
    onBack: goBack,
    onForward: goForward,
    onEmpty: (direction) =>
      showToast(direction === "back" ? "已经是最外一层" : "已经是最新的位置"),
    labels: { back: "← 上一层", forward: "→ 下一层" },
  });

  const goUp = useCallback(() => {
    if (!root || atRoot) return;
    const parent = parentOf(path);
    navigate(isInside(parent, root) ? parent : root);
  }, [atRoot, navigate, path, root]);

  const openItem = useCallback(
    (item: LauncherItem) => {
      if (item.isDir) {
        navigate(item.path);
        return;
      }
      void invoke("launcher_open", { path: item.path }).catch((error) =>
        showToast(String(error)),
      );
    },
    [navigate, showToast],
  );

  const startItemDrag = useCallback(
    (item: LauncherItem, event: React.PointerEvent) => {
      if (event.button !== 0) return;
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
        void invoke("fence_begin_drag", { id, paths: [item.path] })
          .catch((error) => showToast(String(error)))
          .finally(() => scheduleRefresh());
      };
      window.addEventListener("pointermove", move);
      window.addEventListener("pointerup", stop);
    },
    [id, scheduleRefresh, showToast],
  );

  const startHeaderMove = useCallback(
    (event: React.PointerEvent) => {
      if (event.button !== 0) return;
      const startX = event.clientX;
      const startY = event.clientY;
      let started = false;
      const stop = () => {
        window.removeEventListener("pointermove", move);
        window.removeEventListener("pointerup", stop);
      };
      const move = (next: PointerEvent) => {
        if (started) return;
        if (Math.abs(next.clientX - startX) + Math.abs(next.clientY - startY) < 4) return;
        started = true;
        stop();
        void invoke("fence_begin_move", { id }).catch((error) => showToast(String(error)));
      };
      window.addEventListener("pointermove", move);
      window.addEventListener("pointerup", stop);
    },
    [id, showToast],
  );

  const toggleRolled = useCallback(() => {
    if (!state) return;
    const rolled = !state.fence.rolled;
    setState({ ...state, fence: { ...state.fence, rolled } });
    void invoke("fence_update", { id, rolled }).catch((error) => showToast(String(error)));
  }, [id, showToast, state]);

  const beginRename = useCallback(
    (item: LauncherItem) => {
      setMenu(null);
      setRenaming({ path: item.path, name: displayName(item.name) });
      void invoke("fence_editing", { id, editing: true }).catch(() => undefined);
      window.setTimeout(() => renameRef.current?.select(), 30);
    },
    [id],
  );

  const endRename = useCallback(
    (commit: boolean) => {
      const current = renaming;
      if (!current) return;
      setRenaming(null);
      void invoke("fence_editing", { id, editing: false }).catch(() => undefined);
      if (!commit) return;
      const name = current.name.trim();
      const original = displayName(current.path.split(/[\\/]/).pop() ?? "");
      if (!name || name === original) return;
      void invoke("launcher_rename", { path: current.path, name })
        .then(() => scheduleRefresh())
        .catch((error) => showToast(String(error)));
    },
    [id, renaming, scheduleRefresh, showToast],
  );

  const openMenu = useCallback((event: React.MouseEvent) => {
    event.stopPropagation();
    const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
    setMenu({ x: Math.max(6, rect.right - 148), y: rect.bottom + 4 });
  }, []);

  const changeFolder = useCallback(async () => {
    setMenu(null);
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const pickedFolder = await open({ directory: true, multiple: false });
      if (typeof pickedFolder !== "string") return;
      await invoke("fence_update", { id, path: pickedFolder, title: folderLabel(pickedFolder) });
      // 根目录换了，旧历史不再有效。
      backStack.current = [];
      forwardStack.current = [];
      setHistoryCounts({ back: 0, forward: 0 });
      navigate(pickedFolder, false);
    } catch (error) {
      showToast(String(error));
    }
  }, [id, navigate, showToast]);

  // 键盘：Esc 关菜单 / 取消改名 / 退回上一层；Alt+↑ 和 Backspace 也是上一层。
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const inInput =
        event.target instanceof HTMLElement &&
        event.target.closest("input, textarea, [contenteditable='true']");
      if (event.key === "Escape") {
        if (menu) {
          setMenu(null);
          event.preventDefault();
        } else if (renaming) {
          endRename(false);
          event.preventDefault();
        } else if (picked) {
          setPicked(null);
        }
        return;
      }
      if (inInput) return;
      if (event.key === "Backspace" || (event.altKey && event.key === "ArrowUp")) {
        event.preventDefault();
        goUp();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [endRename, goUp, menu, picked, renaming]);

  // 菜单点外面就关。
  useEffect(() => {
    if (!menu) return;
    const close = () => setMenu(null);
    window.addEventListener("pointerdown", close);
    return () => window.removeEventListener("pointerdown", close);
  }, [menu]);

  useEffect(() => {
    if (renaming) renameRef.current?.focus();
  }, [renaming]);
  useEffect(() => {
    if (renamingFence !== null) titleRef.current?.focus();
  }, [renamingFence]);

  const empty = !loading && !loadError && sorted.length === 0;

  return (
    <div className={`fence${state?.fence.rolled ? " rolled" : ""}`}>
      <div
        className="head"
        onPointerDown={(event) => {
          if ((event.target as HTMLElement).closest(".icon-btn, .title-input, .menu, .crumb")) {
            return;
          }
          startHeaderMove(event);
        }}
        onDoubleClick={(event) => {
          if ((event.target as HTMLElement).closest(".icon-btn, .title-input, .menu, .crumb")) {
            return;
          }
          toggleRolled();
        }}
      >
        {!atRoot && (
          <button className="icon-btn" title="上一级" onClick={goUp} aria-label="上一级">
            <svg width="15" height="15" viewBox="0 0 16 16" fill="none" aria-hidden>
              <path
                d="M10 3.5 5.5 8 10 12.5"
                stroke="currentColor"
                strokeWidth="1.5"
                strokeLinecap="round"
                strokeLinejoin="round"
              />
            </svg>
          </button>
        )}
        <span className="folder-glyph" aria-hidden>
          <svg width="17" height="17" viewBox="0 0 20 20" fill="none">
            <path
              d="M2.6 5.4c0-.66.54-1.2 1.2-1.2h3.1l1.5 1.6h7.8c.66 0 1.2.54 1.2 1.2v8.2c0 .66-.54 1.2-1.2 1.2H3.8c-.66 0-1.2-.54-1.2-1.2V5.4Z"
              stroke="currentColor"
              strokeWidth="1.3"
              strokeLinejoin="round"
            />
          </svg>
        </span>
        {renamingFence !== null ? (
          <input
            ref={titleRef}
            className="title-input"
            value={renamingFence}
            onChange={(event) => setRenamingFence(event.target.value)}
            onBlur={() => {
              const value = renamingFence.trim();
              setRenamingFence(null);
              if (value && value !== state?.fence.title) {
                void invoke("fence_update", { id, title: value })
                  .then(() => applyState(false))
                  .catch((error) => showToast(String(error)));
              }
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter") (event.target as HTMLInputElement).blur();
              if (event.key === "Escape") setRenamingFence(null);
            }}
          />
        ) : crumbs.length > 0 ? (
          <div className="crumbs" title={path}>
            <button
              type="button"
              data-crumb={root}
              className={dropHover === root ? "crumb drop-hover" : "crumb"}
              onClick={() => navigate(root)}
            >
              {baseLabel}
            </button>
            {crumbs.map((crumb) => (
              <React.Fragment key={crumb.path}>
                <span className="crumb-sep" aria-hidden>
                  ›
                </span>
                <button
                  type="button"
                  data-crumb={crumb.path}
                  className={dropHover === crumb.path ? "crumb drop-hover" : "crumb"}
                  onClick={() => navigate(crumb.path)}
                >
                  {crumb.name}
                </button>
              </React.Fragment>
            ))}
          </div>
        ) : (
          <span className="title" title={path}>
            {title || "格子"}
          </span>
        )}
        {!renamingFence && <span className="count">{sorted.length}</span>}
        <button
          className="icon-btn"
          title="更多"
          aria-label="更多"
          onClick={openMenu}
        >
          <svg width="15" height="15" viewBox="0 0 16 16" fill="currentColor" aria-hidden>
            <circle cx="3.5" cy="8" r="1.2" />
            <circle cx="8" cy="8" r="1.2" />
            <circle cx="12.5" cy="8" r="1.2" />
          </svg>
        </button>
      </div>

      <div
        className="body"
        ref={bodyRef}
        onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}
        onContextMenu={(event) => {
          // 空白处：系统原生「新建 / 粘贴 / 属性」菜单。
          if ((event.target as HTMLElement).closest(".tile")) return;
          event.preventDefault();
          if (path) {
            void invoke("launcher_background_menu", { path }).catch((error) =>
              showToast(String(error)),
            );
          }
        }}
        // 右键手势：文件区和文件上都能起手（文件会 stopPropagation，所以走捕获阶段）。
        onPointerDownCapture={(event) => gesture.begin(event)}
        onPointerDown={(event) => {
          if (!(event.target as HTMLElement).closest(".tile")) {
            setPicked(null);
            setMenu(null);
          }
        }}
      >
        <div className="space" style={{ height: pad * 2 + totalRows * rowH }}>
          {slice.map((item, index) => {
            const position = firstRow * cols + index;
            const row = Math.floor(position / cols);
            const column = position % cols;
            const key = icons[item.path];
            const dropTarget = dropHover === item.path;
            return (
              <button
                key={item.path}
                className={`tile${picked === item.path ? " picked" : ""}${
                  dropTarget ? " drop-hover" : ""
                }`}
                data-path={item.path}
                data-dir={item.isDir ? "1" : "0"}
                style={{
                  left: pad + column * (cellW + gap),
                  top: pad + row * rowH,
                  width: cellW,
                  height: cellH,
                }}
                title={item.name}
                onClick={(event) => {
                  event.stopPropagation();
                  setPicked(item.path);
                }}
                onDoubleClick={(event) => {
                  event.stopPropagation();
                  openItem(item);
                }}
                onPointerDown={(event) => {
                  event.stopPropagation();
                  setPicked(item.path);
                  startItemDrag(item, event);
                }}
                onContextMenu={(event) => {
                  event.preventDefault();
                  event.stopPropagation();
                  setPicked(item.path);
                  void invoke<string | null>("launcher_context_menu", { path: item.path })
                    .then((choice) => {
                      if (choice === "rename") beginRename(item);
                      else scheduleRefresh();
                    })
                    .catch((error) => showToast(String(error)));
                }}
              >
                <span
                  className="tile-icon"
                  style={{ width: Math.round(32 * z), height: Math.round(32 * z) }}
                >
                  {key ? (
                    <img
                      src={`${ICON_BASE}${key}`}
                      alt=""
                      draggable={false}
                      className="ready"
                    />
                  ) : item.isDir ? (
                    <svg
                      className="tile-glyph"
                      width={Math.round(28 * z)}
                      height={Math.round(28 * z)}
                      viewBox="0 0 20 20"
                      fill="none"
                      aria-hidden
                    >
                      <path
                        d="M2.6 5.4c0-.66.54-1.2 1.2-1.2h3.1l1.5 1.6h7.8c.66 0 1.2.54 1.2 1.2v8.2c0 .66-.54 1.2-1.2 1.2H3.8c-.66 0-1.2-.54-1.2-1.2V5.4Z"
                        stroke="currentColor"
                        strokeWidth="1.2"
                        strokeLinejoin="round"
                      />
                    </svg>
                  ) : (
                    <span
                      className="tile-glyph"
                      style={{ fontSize: Math.round(20 * z) }}
                    >
                      •
                    </span>
                  )}
                </span>
                {renaming?.path === item.path ? (
                  <input
                    ref={renameRef}
                    className="tile-input"
                    value={renaming.name}
                    onChange={(event) =>
                      setRenaming({ path: item.path, name: event.target.value })
                    }
                    onBlur={() => endRename(true)}
                    onKeyDown={(event) => {
                      event.stopPropagation();
                      // Enter 只负责失焦：提交统一走 onBlur，免得改两次名。
                      if (event.key === "Enter") (event.target as HTMLInputElement).blur();
                      if (event.key === "Escape") endRename(false);
                    }}
                  />
                ) : (
                  <span
                    className="tile-name"
                    style={{ fontSize: Math.max(10, Math.round(11 * z)) }}
                  >
                    {displayName(item.name)}
                  </span>
                )}
              </button>
            );
          })}
        </div>
        {loadError && (
          <div className="empty">
            <strong>{isBucket ? "打不开这个收纳盒" : "打不开这个文件夹"}</strong>
            <span>{loadError}</span>
            {!isBucket && <button onClick={changeFolder}>换一个文件夹</button>}
          </div>
        )}
        {empty && !loadError && (
          <div className="empty">
            <strong>这里是空的</strong>
            <span>把文件拖进来，或右键新建</span>
          </div>
        )}
        {loading && !sorted.length && <div className="empty">正在读取…</div>}
      </div>

      {menu && (
        <div
          className="menu"
          style={{ left: menu.x, top: menu.y }}
          onPointerDown={(event) => event.stopPropagation()}
        >
          <button
            onClick={() => {
              setMenu(null);
              setRenamingFence(state?.fence.title ?? title);
            }}
          >
            重命名格子
          </button>
          {!isBucket && <button onClick={changeFolder}>更换文件夹…</button>}
          <button
            onClick={() => {
              setMenu(null);
              void invoke("launcher_open", { path: atRoot ? root : path }).catch((error) =>
                showToast(String(error)),
              );
            }}
          >
            {isBucket ? "打开收纳目录" : "在资源管理器中打开"}
          </button>
          {isBucket && ruleCount > 0 && <button onClick={runRules}>立即整理</button>}
          <div className="sep" />
          {(["name", "modified"] as SortKey[]).map((key) => (
            <button
              key={key}
              onClick={() => {
                setMenu(null);
                const dir =
                  state?.fence.sortKey === key ? (state.fence.sortDir === -1 ? 1 : -1) : 1;
                void invoke("fence_update", { id, sortKey: key, sortDir: dir }).catch((error) =>
                  showToast(String(error)),
                );
              }}
            >
              按{SORT_LABELS[key]}排序
              {state?.fence.sortKey === key
                ? state.fence.sortDir === -1
                  ? " ↓"
                  : " ↑"
                : ""}
            </button>
          ))}
          <div className="sep" />
          <button
            onClick={() => {
              setMenu(null);
              void invoke("fence_update", { id, visible: false }).catch((error) =>
                showToast(String(error)),
              );
            }}
          >
            隐藏格子
          </button>
          <button
            onClick={() => {
              setMenu(null);
              void invoke("fence_delete", { id }).catch((error) => showToast(String(error)));
            }}
          >
            删除格子
          </button>
        </div>
      )}

      {(["n", "s", "e", "w", "ne", "nw", "se", "sw"] as const).map((edge) => (
        <div
          key={edge}
          className={`resize ${edge}`}
          onPointerDown={(event) => {
            if (event.button !== 0) return;
            event.stopPropagation();
            void invoke("fence_begin_resize", { id, edge }).catch((error) =>
              showToast(String(error)),
            );
          }}
        />
      ))}

      {/* 右键手势的轨迹层：fixed 铺满窗口，永远不吃鼠标事件。 */}
      {gesture.layer}

      {toast && <div className="toast">{toast}</div>}
    </div>
  );
}

ReactDOM.createRoot(document.getElementById("root")!).render(<Fence />);
