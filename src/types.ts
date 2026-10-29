export type Side = "left" | "right";
export type Sides = "both" | "left" | "right";
export type Theme = "system" | "light" | "dark";
export type BandPosition = "bottom" | "top" | "both";

export interface TriggerSettings {
  ratio: number;
  position: BandPosition;
  sides: Sides;
  dwellMs: number;
  cooldownMs: number;
  pollMs: number;
  restoreCursor: boolean;
  restoreOffsetPx: number;
}

export interface GuardSettings {
  skipWhenButtonDown: boolean;
  excludeTaskbar: boolean;
  skipWhenLocked: boolean;
  processBlacklist: string[];
  pauseHotkey: string;
}

export interface UiSettings {
  theme: Theme;
  language: string;
  onboarded: boolean;
}

export interface MetaSettings {
  schema: number;
  updatedAt: number;
  deviceId: string;
}

export interface Config {
  enabled: boolean;
  trigger: TriggerSettings;
  guard: GuardSettings;
  hotkeys: HotkeySettings;
  phrases: PhraseSettings;
  launcher: LauncherSettings;
  sync: SyncSettings;
  updates: UpdateSettings;
  ui: UiSettings;
  meta: MetaSettings;
}

export interface UpdateSettings {
  endpoint: string;
  dismissedVersion: string;
}

export interface SyncSettings {
  enabled: boolean;
  url: string;
  username: string;
  password: string;
  lastPush: number;
  lastPull: number;
}

export interface HotkeySettings {
  pause: string;
  phrasesPanel: string;
  launcher: string;
}

export interface Phrase {
  id: string;
  title: string;
  text: string;
}

export interface LauncherSettings {
  enabled: boolean;
  /** 常用文件夹：面板左侧直接列出来，数组顺序就是显示顺序 */
  folders: string[];
  /** 启动器根目录：每个子文件夹是一个分类 */
  root: string;
  /** 分类的显示顺序（按子文件夹名） */
  categoryOrder: string[];
  /** 面板里隐藏的项：分类名，或 __all__ 表示「全部」 */
  hiddenCategories: string[];
  /** 面板里的网址分组 */
  sites: Array<{ name: string; url: string }>;
  /** 内置网页总开关 */
  webEnabled: boolean;
  /** 面板收起后网页保留多少秒（0 = 立即关闭） */
  siteKeepAliveSecs: number;
  /** 网页窗口宽度占工作区百分比 */
  siteWidthPercent: number;
  /** 网页窗口高度占工作区百分比 */
  siteHeightPercent: number;
  /** 网页窗口是否跟随呼出面板（默认固定贴一侧） */
  siteFollowPanel: boolean;
  /** 固定位置时贴哪一侧："left" | "right" */
  siteSide: string;
  /** 呼出时贴着鼠标，而不是屏幕居中 */
  followCursor: boolean;
  /** 面板宽度占工作区百分比 */
  widthPercent: number;
  /** 面板高度占工作区百分比 */
  heightPercent: number;
  /** 呼出时回到上次浏览的位置 */
  rememberLocation: boolean;
  /** 上次停留的位置 */
  lastLocation: { kind: string; base: string; label: string; path: string };
}

export interface PhraseSettings {
  enabled: boolean;
  items: Phrase[];
  target: string;
  targetClass: string;
  targetTitle: string;
  side: "left" | "right";
  inside: boolean;
  panelX: number | null;
  panelY: number | null;
  collapsed: boolean;
  sendOnClick: boolean;
  sendKey: "enter" | "ctrlEnter";
}

export interface MonitorDto {
  index: number;
  x: number;
  y: number;
  w: number;
  h: number;
  primary: boolean;
  device: string;
}

export interface RegionDto {
  monitor: number;
  side: Side;
  edgeX: number;
  y0: number;
  y1: number;
}

export interface HitDto {
  monitor: number;
  side: Side;
  edgeX: number;
}

export interface Point {
  x: number;
  y: number;
}

export interface RunningApp {
  name: string;
  title: string;
  class?: string;
}

export interface Stats {
  ticks: number;
  fires: number;
  failures: number;
}

export interface Snapshot {
  config: Config;
  monitors: MonitorDto[];
  regions: RegionDto[];
  cursor: Point | null;
  hit: HitDto | null;
  guard: string | null;
  paused: boolean;
  testMode: boolean;
  stats: Stats;
  version: string;
  configPath: string;
  autostart: boolean;
  desktopCount: number | null;
  enginePhase: string;
  accent: string | null;
  hotkeyError: string | null;
}
