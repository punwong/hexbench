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
  hotkeys: { pause: "Ctrl+Alt+Shift+P", phrasesPanel: "Ctrl+Alt+Shift+Q" },
  updates: { endpoint: "", dismissedVersion: "" },
  sync: {
    enabled: false,
    url: "",
    username: "",
    password: "",
    lastPush: 0,
    lastPull: 0,
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
