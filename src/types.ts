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
}

export interface Phrase {
  id: string;
  title: string;
  text: string;
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
