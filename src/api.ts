import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Config, HitDto, RunningApp, Snapshot } from "./types";

export const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

const demoConfig: Config = {
  enabled: true,
  trigger: {
    ratio: 0.1,
    position: "bottom",
    sides: "both",
    dwellMs: 120,
    cooldownMs: 500,
    pollMs: 50,
    restoreCursor: true,
    restoreOffsetPx: 24,
  },
  guard: {
    skipWhenButtonDown: true,
    excludeTaskbar: true,
    skipWhenLocked: true,
    processBlacklist: [],
    pauseHotkey: "Ctrl+Alt+Shift+P",
  },
  hotkeys: { pause: "Ctrl+Alt+Shift+P", phrasesPanel: "Ctrl+Alt+Shift+Q", launcher: "Ctrl+Alt+Space" },
  updates: { endpoint: "", dismissedVersion: "" },
  sync: {
    enabled: false,
    url: "",
    username: "",
    password: "",
    lastPush: 0,
    lastPull: 0,
  },
  launcher: {
      enabled: true,
      folders: [],
      root: "",
      categoryOrder: [],
      hiddenCategories: ["__all__"],
      sites: [],
      webEnabled: true,
      siteKeepAliveSecs: 90,
    siteWidthPercent: 24,
    siteHeightPercent: 85,
    siteRememberPosition: true,
    siteFollowPanel: false,
    siteSide: "right",
      followCursor: true,
      widthPercent: 50,
      heightPercent: 50,
      rememberLocation: true,
      lastLocation: { kind: "", base: "", label: "", path: "" },
    },
  phrases: {
    enabled: true,
    items: [
      { id: "p1", title: "在的", text: "亲，在的，有什么可以帮您～" },
      { id: "p2", title: "稍等", text: "稍等，我帮您查一下。" },
    ],
    target: "",
    targetClass: "",
    targetTitle: "",
    side: "right",
    inside: false,
    panelX: null,
    panelY: null,
    collapsed: false,
    sendOnClick: false,
    sendKey: "enter",
  },
  ui: { theme: "system", language: "auto", onboarded: false },
  meta: { schema: 1, updatedAt: 0, deviceId: "" },
};

let mockConfig = demoConfig;
let mockPaused = false;
let mockTest = true;
let mockAutostart = false;
let mockFires = 0;
let mockDesktops = 1;
const started = Date.now();

function mockSnapshot(): Snapshot {
  const t = (Date.now() - started) / 1000;
  // A slow figure-eight, parking on the portrait's left edge every other 8s.
  const onEdge = mockTest && Math.floor(t / 8) % 2 === 1;
  const cursor = onEdge
    ? { x: -1080, y: 1600 }
    : { x: Math.round(1720 + Math.cos(t * 0.6) * 1500), y: Math.round(720 + Math.sin(t * 1.2) * 600) };

  const monitors = [
    { index: 0, x: 0, y: 0, w: 3440, h: 1440, primary: true, device: "DISPLAY1" },
    { index: 1, x: -1080, y: -232, w: 1080, h: 1920, primary: false, device: "DISPLAY2" },
  ];
  const regions = [
    { monitor: 0, side: "right" as const, edgeX: 3439, y0: 1296, y1: 1440 },
    { monitor: 1, side: "left" as const, edgeX: -1080, y0: 1496, y1: 1688 },
    { monitor: 1, side: "right" as const, edgeX: -1, y0: 1496, y1: 1688 },
  ];
  const hit = onEdge ? { monitor: 1, side: "left" as const, edgeX: -1080 } : null;

  return {
    config: mockConfig,
    monitors,
    regions,
    cursor,
    hit,
    guard: mockPaused ? "paused" : null,
    paused: mockPaused,
    testMode: mockTest,
    stats: { ticks: Math.round(t * 20), fires: mockFires, failures: 0 },
    version: "0.1.0",
    configPath: "C:\\Users\\you\\AppData\\Roaming\\io.github.hexbench\\config.json",
    autostart: mockAutostart,
    desktopCount: mockDesktops,
    enginePhase: hit ? "armed" : "idle",
    accent: null,
    hotkeyError: null,
  };
}

export async function getSnapshot(): Promise<Snapshot> {
  if (!isTauri) return mockSnapshot();
  return invoke<Snapshot>("snapshot");
}

export async function saveConfig(config: Config): Promise<void> {
  if (!isTauri) {
    mockConfig = config;
    return;
  }
  await invoke("save_config", { config });
}

export async function setPaused(paused: boolean): Promise<void> {
  if (!isTauri) {
    mockPaused = paused;
    return;
  }
  await invoke("set_paused", { paused });
}

/** Hit + guard, polled by the layout diagram so the highlight is immediate. */
export async function getHitState(): Promise<{
  hit: HitDto | null;
  guard: string | null;
  phase: string;
}> {
  if (!isTauri) return { hit: null, guard: null, phase: "idle" };
  const [hit, guard, phase] = await invoke<[HitDto | null, string | null, string]>("hit_state");
  return { hit, guard, phase };
}

// ---------- 中转站 ----------

/** 用系统默认程序打开一个路径。 */
export async function openPath(path: string): Promise<void> {
  if (!isTauri) return;
  await invoke("launcher_open", { path });
}

/** 系统文件夹选择器（仅桌面端）。 */
export async function pickFolder(): Promise<string | null> {
  if (!isTauri) return null;
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({ directory: true, multiple: false });
  return typeof picked === "string" ? picked : null;
}

/** Puts staged files on the clipboard as real files (Ctrl+V pastes them). */
/** Hands a staged file to the OS so the user can drag it out of the window. */
/** Fires when files are dropped onto the window. */
export async function closePanel(): Promise<void> {
  if (!isTauri) return;
  await invoke("panel_close");
}

export async function setTestMode(enabled: boolean): Promise<void> {
  if (!isTauri) {
    mockTest = enabled;
    return;
  }
  await invoke("set_test_mode", { enabled });
}

export async function setAutostart(enabled: boolean): Promise<void> {
  if (!isTauri) {
    mockAutostart = enabled;
    return;
  }
  await invoke("set_autostart", { enabled });
}

export async function openConfigFolder(): Promise<void> {
  if (!isTauri) return;
  await invoke("open_config_folder");
}

export async function refreshLayout(): Promise<void> {
  if (!isTauri) return;
  await invoke("refresh_layout");
}

export async function createVirtualDesktop(): Promise<void> {
  if (!isTauri) {
    mockDesktops += 1;
    return;
  }
  await invoke("create_virtual_desktop");
}

export async function showTriggerOverlay(): Promise<void> {
  if (!isTauri) return;
  await invoke("show_trigger_overlay");
}

const MOCK_APPS: RunningApp[] = [
  { name: "mstsc.exe", title: "远程桌面连接" },
  { name: "msedge.exe", title: "Microsoft Edge" },
  { name: "Code.exe", title: "Visual Studio Code" },
  { name: "explorer.exe", title: "文件资源管理器" },
];

export async function runningApps(): Promise<RunningApp[]> {
  if (!isTauri) return MOCK_APPS;
  return invoke<RunningApp[]>("running_apps");
}

export async function togglePhrasesPanel(): Promise<void> {
  if (!isTauri) return;
  await invoke("toggle_phrases_panel");
}

export async function syncTest(
  url: string,
  username: string,
  password: string,
): Promise<void> {
  if (!isTauri) return;
  await invoke("sync_test", { url, username, password });
}

export async function syncSave(
  url: string,
  username: string,
  password: string,
  enabled: boolean,
): Promise<void> {
  if (!isTauri) return;
  await invoke("sync_save", { url, username, password, enabled });
}

export async function syncPush(): Promise<string> {
  if (!isTauri) return "浏览器里不支持";
  return invoke<string>("sync_push");
}

export async function syncPull(): Promise<string> {
  if (!isTauri) return "浏览器里不支持";
  return invoke<string>("sync_pull");
}

export interface UpdateInfo {
  version: string;
  date: string;
  notes: string;
}

export async function checkUpdate(): Promise<UpdateInfo | null> {
  if (!isTauri) return null;
  return invoke<UpdateInfo | null>("update_check");
}

export async function downloadUpdate(
  onProgress: (percent: number | null) => void,
): Promise<void> {
  if (!isTauri) return;
  let downloaded = 0;
  let total: number | null = null;
  const unlisten = await listen<{ chunk: number; total: number | null }>(
    "update-progress",
    (event) => {
      downloaded += event.payload.chunk;
      total = event.payload.total ?? total;
      onProgress(total ? Math.min(100, Math.round((downloaded / total) * 100)) : null);
    },
  );
  try {
    // Only fetches the installer; restarting is a separate, explicit step.
    await invoke<UpdateInfo>("update_download");
  } finally {
    unlisten();
  }
}

export async function installDownloaded(): Promise<void> {
  if (!isTauri) return;
  await invoke("update_install");
}


export interface UpdateStatus {
  version: string | null;
  error: string | null;
  checkedAt: number;
}

/// Cached result of the process-wide check (no network round trip).
export async function updateStatus(): Promise<UpdateStatus> {
  if (!isTauri) return { version: null, error: null, checkedAt: 0 };
  return invoke<UpdateStatus>("update_status");
}

export async function onUpdateStatus(
  handler: (status: UpdateStatus) => void,
): Promise<() => void> {
  if (!isTauri) return () => undefined;
  return listen<UpdateStatus>("update-status", (event) => handler(event.payload));
}

/// Full diagnostic dump (runtime state, monitors, config, log tail) for bug reports.
export async function diagnostics(): Promise<string> {
  if (!isTauri) return "浏览器预览模式，没有诊断信息";
  return invoke<string>("diagnostics");
}

export async function copyText(text: string): Promise<void> {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(text);
    return;
  }
  // Fallback for webviews without the async clipboard API.
  const area = document.createElement("textarea");
  area.value = text;
  area.style.position = "fixed";
  area.style.opacity = "0";
  document.body.appendChild(area);
  area.select();
  document.execCommand("copy");
  area.remove();
}

export async function pickApp(): Promise<void> {
  if (!isTauri) return;
  await invoke("pick_app");
}

export async function onAppPicked(
  handler: (app: RunningApp) => void,
): Promise<() => void> {
  if (!isTauri) return () => undefined;
  return listen<RunningApp>("app-picked", (event) => handler(event.payload));
}

/// 设置页「缓存」用到的信息。
export type CacheInfo = {
  key: string;
  label: string;
  hint: string;
  path: string;
  bytes: number;
  files: number;
};

export async function cacheStats(): Promise<CacheInfo[]> {
  if (!isTauri) return [];
  return invoke<CacheInfo[]>("cache_stats");
}

export async function cacheClear(): Promise<number> {
  if (!isTauri) return 0;
  return invoke<number>("cache_clear");
}
/// 保存「常用文件夹」的完整列表（增删、排序都走这里）。
export async function saveLauncherFolders(folders: string[]): Promise<void> {
  if (!isTauri) return;
  await invoke("launcher_save_folders", { folders });
}

/// 保存分类的显示顺序。
export async function saveCategoryOrder(order: string[]): Promise<void> {
  if (!isTauri) return;
  await invoke("launcher_save_category_order", { order });
}
export interface LauncherCategoryInfo {
  name: string;
  path: string;
  count: number;
  /** 面板里是否隐藏 */
  hidden: boolean;
}

/// 某个目录下的分类（顺序已经按用户排好的来）。
///
/// 直接传路径：设置页要的是「这个文件夹里有什么」，不是「当前配置的根目录」。
export async function launcherCategoryList(path: string): Promise<LauncherCategoryInfo[]> {
  if (!isTauri || !path) return [];
  return invoke<LauncherCategoryInfo[]>("launcher_categories_at", { path });
}
/// 抓网页标题，用来给网址自动填名字。
export async function siteTitle(url: string): Promise<string> {
  if (!isTauri || !url.trim()) return "";
  return invoke<string>("site_title", { url });
}