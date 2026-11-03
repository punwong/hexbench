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
  launcher: LauncherSettings;
  /** 收藏（笔记）：呼出面板里的 Markdown 笔记和图片 */
  notes: NotesSettings;
  /** 桌面格子：桌面上的文件夹小组件 */
  desktop: DesktopSettings;
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
  launcher: string;
}

/** 面板文件区的右键滑动手势。 */
export interface GestureSettings {
  /** 手势总开关 */
  enabled: boolean;
  /** 左右互换：true 时向左是前进 */
  invertDirection: boolean;
  /** 横向滑多远才执行（逻辑像素） */
  triggerDistance: number;
  /** 拖动时画轨迹线、显示方向 */
  showTrail: boolean;
}

/** 格子的来源：绑定的文件夹，还是应用自己建的收纳盒。 */
export type FenceKind = "folder" | "bucket";

/** 一条整理规则：从源目录里挑符合条件的文件收进收纳盒。 */
export interface FenceRule {
  id: string;
  /** 规则名，也用作「收进这个子目录」时的默认目录名 */
  name: string;
  enabled: boolean;
  /** 扩展名白名单（小写、不带点）；空 = 不限 */
  exts: string[];
  /** 文件名里要包含的文字；空 = 不限 */
  nameContains: string;
  /** 大小上下限（MB，0 = 不限） */
  minMb: number;
  maxMb: number;
  /** 只收最后修改早于 N 天的文件（0 = 不限） */
  olderThanDays: number;
  /** 监视的来源目录（通常填桌面 / 下载） */
  sources: string[];
  /** 收进盒子的哪个子目录；空 = 盒子根 */
  subdir: string;
}

/** 一个桌面格子：几何按显示器工作区归一化。 */
export interface FenceSettings {
  id: string;
  title: string;
  path: string;
  kind: FenceKind;
  /** 整理规则（只有收纳盒用得上） */
  rules: FenceRule[];
  /** 源目录一有变化就自动整理 */
  rulesAuto: boolean;
  monitor: string;
  /** 工作区内的归一化矩形（0..1） */
  x: number;
  y: number;
  w: number;
  h: number;
  /** 上次的物理矩形（像素），显示器找不到时兜底 */
  pixelX: number;
  pixelY: number;
  pixelW: number;
  pixelH: number;
  rolled: boolean;
  visible: boolean;
  zoom: number;
  sortKey: string;
  sortDir: number;
}

export interface DesktopSettings {
  enabled: boolean;
  keepOnShowDesktop: boolean;
  snap: boolean;
  snapGrid: boolean;
  /** 桌面右键菜单里挂一个 HexBench 子菜单（本机注册表状态） */
  contextMenu: boolean;
  /** 卡片不透明度百分比（30–100）：越小背后透得越多 */
  opacity: number;
  /** 背后要不要磨砂；关掉就是「透而不糊」 */
  frosted: boolean;
  /** Beta：格子改由原生渲染层绘制（关掉回到 WebView） */
  native: boolean;
  fences: FenceSettings[];
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
  /** 是否记住网页窗口拖动后的位置和大小 */
  siteRememberPosition: boolean;
  /** 网页窗口是否跟随呼出面板（默认固定贴一侧） */
  siteFollowPanel: boolean;
  /** 固定位置时贴哪一侧："left" | "right" */
  siteSide: string;
  /** 呼出时贴着鼠标，而不是屏幕居中 */
  followCursor: boolean;
  /** 面板宽度占工作区百分比 */
  widthPercent: number;
  /** 是否记住拖出来的面板大小 */
  panelRememberSize: boolean;
  /** 面板高度占工作区百分比 */
  heightPercent: number;
  /** 呼出时回到上次浏览的位置 */
  rememberLocation: boolean;
  /** 上次停留的位置 */
  lastLocation: { kind: string; base: string; label: string; path: string };
  /** 文件区右键滑动手势 */
  gestures: GestureSettings;
}

export interface NotesSettings {
  enabled: boolean;
  /** 空 = 默认的「文档\HexBench 收藏」 */
  folder: string;
  /** 发送后是否再按一次回车 */
  sendEnter: boolean;
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
