import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import {
  Crosshair,
  ExternalLink,
  Eye,
  EyeOff,
  FolderOpen,
  Github,
  Inbox,
  LayoutGrid,
  List,
  Lock,
  LockOpen,
  Plus,
  Rocket,
  Scale,
  ScrollText,
  SlidersHorizontal,
  Trash2,
} from "lucide-react";

import type { CacheInfo, LauncherCategoryInfo } from "./api";
import type {
  Config,
  DesktopSettings,
  FenceRule,
  FenceSettings,
  GestureSettings,
  GuardSettings,
  HotkeySettings,
  LauncherSettings,
  MonitorDto,
  NotesSettings,
  RunningApp,
  Snapshot,
  TriggerSettings,
  UiSettings,
  UpdateSettings,
} from "./types";
import type { FenceBoxInfo, KnownFolders } from "./api";
import {
  cacheClear,
  cacheStats,
  copyText,
  fenceBoxInfo,
  fenceCreate,
  fenceCreateBox,
  fenceDelete,
  fenceKnownFolders,
  fenceRulePreview,
  fenceRuleRun,
  fenceSetAppearance,
  fenceUpdate,
  launcherCategoryList,
  notesClearAll,
  notesDefaultFolder,
  notesOpenFolder,
  siteTitle,
  diagnostics,
  onAppPicked,
  openPath,
  openUrl,
  pickApp,
  pickFolder,
  runningApps,
  syncPull,
  syncPush,
  syncSave,
  syncTest,
} from "./api";
import { TriggerPreview } from "./components/TriggerPreview";
import {
  Card,
  ConfirmDialog,
  EmptyState,
  NumberField,
  Row,
  Segmented,
  Slider,
  Tabs,
  Toggle,
} from "./components/ui";

export type Patch = {
  enabled?: boolean;
  trigger?: Partial<TriggerSettings>;
  guard?: Partial<GuardSettings>;
  hotkeys?: Partial<HotkeySettings>;
  launcher?: Partial<LauncherSettings>;
  notes?: Partial<NotesSettings>;
  desktop?: Partial<DesktopSettings>;
  updates?: Partial<UpdateSettings>;
  ui?: Partial<UiSettings>;
};

export type UpdateConfig = (
  patch: Patch | ((current: Config) => Patch | null),
) => void;

function ToolRow({
  icon,
  name,
  hint,
  keycap,
  enabled,
  onToggle,
  onOpen,
}: {
  icon: ReactNode;
  name: string;
  hint: string;
  keycap?: string;
  enabled: boolean;
  onToggle: (value: boolean) => void;
  onOpen: () => void;
}) {
  return (
    <div className="tool">
      <span className="tool-icon">{icon}</span>
      <span className="tool-text">
        <span className="tool-name">
          {name}
          {keycap && <kbd>{keycap}</kbd>}
        </span>
        <span className="tool-hint">{hint}</span>
      </span>
      <button type="button" className="btn" onClick={onOpen}>
        设置
      </button>
      <Toggle checked={enabled} onChange={onToggle} />
    </div>
  );
}

export function Overview({
  snap,
  config,
  update,
  onOpen,
}: {
  snap: Snapshot;
  config: Config;
  update: UpdateConfig;
  onOpen: (page: "edge" | "launcher" | "desktop") => void;
}) {
  const desktops = snap.desktopCount;
  return (
    <div className="stack">
      <Card title="工具">
        <div className="tools">
          <ToolRow
            icon={<Rocket size={18} />}
            name="快捷面板"
            hint="呼出面板，找文件、搬文件、写便签"
            keycap={config.hotkeys.launcher}
            enabled={config.launcher.enabled}
            onToggle={(value) => update({ launcher: { enabled: value } })}
            onOpen={() => onOpen("launcher")}
          />

          <ToolRow
            icon={<Crosshair size={18} />}
            name="边缘桌面"
            hint={
              desktops
                ? `鼠标贴到屏幕最外侧边缘切换虚拟桌面（当前 ${desktops} 个）`
                : "鼠标贴到屏幕最外侧边缘切换虚拟桌面"
            }
            enabled={config.enabled}
            onToggle={(value) => update({ enabled: value })}
            onOpen={() => onOpen("edge")}
          />
          <ToolRow
            icon={<LayoutGrid size={18} />}
            name="桌面格子"
            hint="把文件夹钉在桌面上，随时取用"
            enabled={config.desktop.enabled}
            onToggle={(value) => update({ desktop: { enabled: value } })}
            onOpen={() => onOpen("desktop")}
          />
        </div>
      </Card>
    </div>
  );
}

export function EdgePage({
  config,
  update,
  monitors,
  onPreview,
}: {
  config: Config;
  update: UpdateConfig;
  monitors: MonitorDto[];
  onPreview: () => void;
}) {
  const [tab, setTab] = useState<"trigger" | "guards">("trigger");

  return (
    <div className="stack">
      <Card>
        <Row label="启用边缘切换">
          <Toggle
            checked={config.enabled}
            onChange={(value) => update({ enabled: value })}
          />
        </Row>
      </Card>
      <Tabs
        value={tab}
        options={[
          { value: "trigger", label: "触发" },
          { value: "guards", label: "防误触" },
        ]}
        onChange={setTab}
      />
      {tab === "trigger" ? (
        <>
          <TriggerPage
            config={config}
            update={update}
            monitors={monitors}
            onPreview={onPreview}
          />
        </>
      ) : (
        <GuardsPage config={config} update={update} />
      )}
    </div>
  );
}
export function TriggerPage({
  config,
  update,
  monitors,
  onPreview,
}: {
  config: Config;
  update: UpdateConfig;
  monitors: MonitorDto[];
  onPreview: () => void;
}) {
  const trigger = config.trigger;
  const positionLabel =
    trigger.position === "bottom" ? "底部" : trigger.position === "top" ? "顶部" : "上下";
  const sidesLabel =
    trigger.sides === "both" ? "两侧" : trigger.sides === "left" ? "仅左" : "仅右";
  const percent = Math.round(trigger.ratio * 100);
  const corners = trigger.position === "both" && trigger.sides === "both";

  return (
    <div className="stack">
      <Card
        title="预览"
        action={
          <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
            <span className="sub">
              {positionLabel} {percent}% · {sidesLabel}
              {corners ? "（四角）" : ""}
            </span>
            <button
              type="button"
              className="btn"
              title="在屏幕上显示 5 秒"
              onClick={onPreview}
            >
              在屏幕上预览
            </button>
          </div>
        }
      >
        <TriggerPreview monitors={monitors} trigger={trigger} />
        <div className="legend">
          <span>
            <i className="swatch area" />
            触发带
          </span>
          <span>
            <i className="swatch edge" />
            可触发边缘
          </span>
        </div>
        <p className="sub" style={{ marginTop: 10, marginBottom: 0 }}>
          屏幕拼接处的接缝不触发。
        </p>
      </Card>

      <Card title="触发带">
        <Row label="贴哪条边" hint="选「上下」再配「两侧」就是四个角">
          <Segmented
            value={config.trigger.position}
            options={[
              { value: "bottom", label: "底部" },
              { value: "top", label: "顶部" },
              { value: "both", label: "上下" },
            ]}
            onChange={(value) => update({ trigger: { position: value } })}
          />
        </Row>
        <Row label="左右两侧">
          <Segmented
            value={config.trigger.sides}
            options={[
              { value: "both", label: "两侧" },
              { value: "left", label: "仅左" },
              { value: "right", label: "仅右" },
            ]}
            onChange={(value) => update({ trigger: { sides: value } })}
          />
        </Row>
        <Row label="占屏高比例" hint="每块屏按自己的高度计算">
          <Slider
            value={Math.round(config.trigger.ratio * 100)}
            min={3}
            max={50}
            step={1}
            onChange={(value) => update({ trigger: { ratio: value / 100 } })}
            format={(value) => `${value}%`}
          />
        </Row>
      </Card>

      {/* 调好触发带就够用了；时序、光标回位、轮询这些平时不用动，收起来。 */}
      <Card>
        <details className="adv-details">
          <summary>高级：时序与光标</summary>
          <Row label="停留阈值" hint="光标在触发带里停多久才切换">
            <NumberField
              value={config.trigger.dwellMs}
              min={0}
              max={500}
              step={10}
              suffix="ms"
              onChange={(value) => update({ trigger: { dwellMs: value } })}
            />
          </Row>
          <Row label="冷却" hint="两次触发的最小间隔">
            <NumberField
              value={config.trigger.cooldownMs}
              min={200}
              max={2000}
              step={50}
              suffix="ms"
              onChange={(value) => update({ trigger: { cooldownMs: value } })}
            />
          </Row>
          <Row label="触发后向内推" hint="同时充当「离开触发区」，避免连续切换">
            <Toggle
              checked={config.trigger.restoreCursor}
              onChange={(value) => update({ trigger: { restoreCursor: value } })}
            />
          </Row>
          <Row label="内推距离">
            <NumberField
              value={config.trigger.restoreOffsetPx}
              min={8}
              max={200}
              step={4}
              suffix="px"
              onChange={(value) => update({ trigger: { restoreOffsetPx: value } })}
            />
          </Row>
          <Row label="轮询间隔" hint="越小越跟手，也越费电">
            <NumberField
              value={config.trigger.pollMs}
              min={20}
              max={100}
              step={10}
              suffix="ms"
              onChange={(value) => update({ trigger: { pollMs: value } })}
            />
          </Row>
        </details>
      </Card>
    </div>
  );
}

function AppPicker({
  title,
  hint,
  onPick,
  onClose,
}: {
  title: string;
  hint: string;
  onPick: (app: RunningApp) => void;
  onClose: () => void;
}) {
  const [apps, setApps] = useState<RunningApp[]>([]);
  const [query, setQuery] = useState("");
  const [picking, setPicking] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void runningApps()
      .then(setApps)
      .catch(() => setApps([]));
  }, []);

  useEffect(() => {
    if (!picking) return;
    let stop: (() => void) | undefined;
    let alive = true;
    void onAppPicked((app) => {
      setPicking(false);
      setError(null);
      onPick(app);
    }).then((unlisten) => {
      if (alive) {
        stop = unlisten;
      } else {
        unlisten();
      }
    });
    void pickApp().catch((reason) => {
      setPicking(false);
      setError(String(reason));
    });
    return () => {
      alive = false;
      stop?.();
    };
  }, [picking, onPick]);

  const needle = query.trim().toLowerCase();
  const filtered = apps.filter((app) =>
    `${app.name} ${app.title}`.toLowerCase().includes(needle),
  );

  return (
    <div className="modal-backdrop" role="dialog" aria-modal="true" aria-label={title}>
      <div className="modal">
        <h2>{title}</h2>
        <p>{hint}</p>
        <input
          autoFocus
          className="search"
          value={query}
          placeholder="搜索进程名或窗口标题…"
          onChange={(event) => setQuery(event.target.value)}
        />
        <div className="app-list">
          {filtered.length === 0 ? (
            <div className="sub">没有匹配的应用</div>
          ) : (
            filtered.map((app) => (
              <button
                key={app.name}
                type="button"
                className="app-item"
                onClick={() => onPick(app)}
              >
                <span className="app-name">{app.name}</span>
                <span className="app-title">{app.title}</span>
              </button>
            ))
          )}
        </div>
        {picking && <p className="sub">现在点一下屏幕上的目标窗口…</p>}
        {error && <p className="sub">{error}</p>}
        <div className="modal-actions">
          <button
            type="button"
            className="btn"
            disabled={picking}
            onClick={() => setPicking(true)}
          >
            拾取窗口
          </button>
          <button type="button" className="btn" onClick={onClose}>
            关闭
          </button>
        </div>
      </div>
    </div>
  );
}

export function GuardsPage({
  config,
  update,
}: {
  config: Config;
  update: UpdateConfig;
}) {
  const [picker, setPicker] = useState(false);
  const blacklist = config.guard.processBlacklist;

  const openPicker = () => setPicker(true);

  const addApp = (name: string) => {
    if (!blacklist.includes(name)) {
      update({ guard: { processBlacklist: [...blacklist, name] } });
    }
    setPicker(false);
  };

  const removeApp = (name: string) => {
    update({ guard: { processBlacklist: blacklist.filter((entry) => entry !== name) } });
  };

  return (
    <div className="stack">
      <Card title="什么时候不触发">
        <Row label="鼠标按键按下时" hint="拖拽、框选、拖窗口都不误触">
          <Toggle
            checked={config.guard.skipWhenButtonDown}
            onChange={(value) => update({ guard: { skipWhenButtonDown: value } })}
          />
        </Row>

        <Row label="光标在任务栏上" hint="开始按钮、托盘、显示桌面都不触发">
          <Toggle
            checked={config.guard.excludeTaskbar}
            onChange={(value) => update({ guard: { excludeTaskbar: value } })}
          />
        </Row>
        <Row label="锁屏时">
          <Toggle
            checked={config.guard.skipWhenLocked}
            onChange={(value) => update({ guard: { skipWhenLocked: value } })}
          />
        </Row>
      </Card>

      <Card
        title="进程黑名单"
        action={
          <button type="button" className="btn" onClick={openPicker}>
            添加应用
          </button>
        }
      >
        {blacklist.length === 0 ? (
          <p className="sub" style={{ margin: 0 }}>
            这些程序在前台时不触发，比如远程桌面、全屏游戏、模拟器。
          </p>
        ) : (
          <div className="chips">
            {blacklist.map((name) => (
              <span className="chip" key={name}>
                {name}
                <button
                  type="button"
                  aria-label={`移除 ${name}`}
                  onClick={() => removeApp(name)}
                >
                  ×
                </button>
              </span>
            ))}
          </div>
        )}
      </Card>

      {picker && (
        <AppPicker
          title="添加应用"
          hint="点一下加入黑名单，或用「拾取窗口」点屏幕上的程序。"
          onPick={(app) => addApp(app.name)}
          onClose={() => setPicker(false)}
        />
      )}
    </div>
  );
}

function SyncCard({ config }: { config: Config }) {
  const [url, setUrl] = useState(config.sync.url);
  const [user, setUser] = useState(config.sync.username);
  const [pass, setPass] = useState("");
  const [status, setStatus] = useState("");
  const [busy, setBusy] = useState(false);
  // 上传 / 拉取都是一键覆盖：先问一句，并写明会覆盖哪一边。
  const [ask, setAsk] = useState<null | "push" | "pull">(null);

  useEffect(() => {
    setUrl(config.sync.url);
    setUser(config.sync.username);
  }, [config.sync.url, config.sync.username]);

  const run = async (label: string, action: () => Promise<string | void>) => {
    setBusy(true);
    setStatus(`${label}…`);
    try {
      const message = await action();
      setStatus(typeof message === "string" ? message : `${label}成功`);
    } catch (reason) {
      setStatus(String(reason));
    } finally {
      setBusy(false);
    }
  };

  const remember = () => syncSave(url, user, pass, true);

  return (
    <Card title="配置同步（WebDAV）">
      <p className="sub" style={{ marginTop: 0 }}>
        密码只在本机加密保存，不会上传。
      </p>
      <Row label="地址" hint="指向一个已存在的目录，例如 https://dav.example.com/hexbench/">
        <input
          className="phrase-text"
          style={{ width: 330 }}
          value={url}
          placeholder="https://…/hexbench/"
          onChange={(event) => setUrl(event.target.value)}
        />
      </Row>
      <Row label="用户名">
        <input
          className="phrase-text"
          style={{ width: 220 }}
          value={user}
          onChange={(event) => setUser(event.target.value)}
        />
      </Row>
      <Row label="密码">
        <input
          className="phrase-text"
          style={{ width: 220 }}
          type="password"
          value={pass}
          placeholder={config.sync.password ? "已保存，留空不修改" : ""}
          onChange={(event) => setPass(event.target.value)}
        />
      </Row>
      <div style={{ display: "flex", gap: 8, marginTop: 12, flexWrap: "wrap" }}>
        <button
          type="button"
          className="btn"
          disabled={busy}
          onClick={() => run("测试", () => syncTest(url, user, pass))}
        >
          测试连接
        </button>
        <button
          type="button"
          className="btn"
          disabled={busy}
          onClick={() => setAsk("push")}
        >
          上传配置
        </button>
        <button
          type="button"
          className="btn"
          disabled={busy}
          onClick={() => setAsk("pull")}
        >
          拉取配置
        </button>
        <button
          type="button"
          className="btn"
          disabled={busy}
          onClick={() => run("保存", async () => (await remember(), await syncSave(url, user, pass, true)))}
        >
          保存
        </button>
      </div>
      {status && <p className="sub" style={{ marginBottom: 0 }}>{status}</p>}
      <ConfirmDialog
        open={ask !== null}
        title={ask === "pull" ? "用云端配置覆盖本机？" : "用本机配置覆盖云端？"}
        confirmLabel={ask === "pull" ? "覆盖本机" : "覆盖云端"}
        onCancel={() => setAsk(null)}
        onConfirm={() => {
          const mode = ask;
          setAsk(null);
          if (mode === "pull") {
            void run("拉取", async () => (await remember(), await syncPull()));
          } else if (mode === "push") {
            void run("上传", async () => (await remember(), await syncPush()));
          }
        }}
      >
        {ask === "pull"
          ? "云端的设置会写进这台电脑，本机现在的设置会被替换。"
          : "云端已有的配置会被这台电脑的设置替换。"}
      </ConfirmDialog>
    </Card>
  );
}

export function GeneralPage({
  config,
  update,
  snap,
  onAutostart,
  onRefresh,
  onOpenFolder,
}: {
  config: Config;
  update: UpdateConfig;
  snap: Snapshot;
  onAutostart: (value: boolean) => void;
  onRefresh: () => void;
  onOpenFolder: () => void;
}) {
  return (
    <div className="stack">
      <Card title="常规">
        <Row label="开机自动启动" hint="只进托盘，不弹窗">
          <Toggle checked={snap.autostart} onChange={onAutostart} />
        </Row>
        <Row label="主题">
          <Segmented
            value={config.ui.theme}
            options={[
              { value: "system", label: "跟随系统" },
              { value: "light", label: "浅色" },
              { value: "dark", label: "深色" },
            ]}
            onChange={(value) => update({ ui: { theme: value } })}
          />
        </Row>
      </Card>

      <Card title="显示器">
        <Row label="当前布局">
          <span className="sub">
            {snap.monitors.map((m) => `${m.w}×${m.h}`).join(" + ") || "—"}
          </span>
        </Row>
        <Row label="重新读取布局" hint="插拔显示器后如果没自动刷新，点一下">
          <button type="button" className="btn" onClick={onRefresh}>
            刷新
          </button>
        </Row>
      </Card>

      <StorageCard />

      <SyncCard config={config} />

      <Card title="配置">
        <div className="mono" style={{ wordBreak: "break-all", color: "var(--muted)" }}>
          {snap.configPath}
        </div>
        <div style={{ marginTop: 12 }}>
          <button type="button" className="btn" onClick={onOpenFolder}>
            打开所在文件夹
          </button>
        </div>
      </Card>
    </div>
  );
}

function formatCheckedAt(ms: number): string {
  const date = new Date(ms);
  const now = new Date();
  const time = `${String(date.getHours()).padStart(2, "0")}:${String(date.getMinutes()).padStart(2, "0")}`;
  if (date.toDateString() === now.toDateString()) return `今天 ${time}`;
  const yesterday = new Date(now);
  yesterday.setDate(now.getDate() - 1);
  if (date.toDateString() === yesterday.toDateString()) return `昨天 ${time}`;
  return `${date.getMonth() + 1}月${date.getDate()}日 ${time}`;
}

type UpdateView = {
  phase: string;
  version: string | null;
  progress: number | null;
  error: string | null;
  notes: string;
  date: string;
  checkedAt: number | null;
};

function UpdateCard({
  state,
  onCheck,
  onDownload,
  onInstall,
}: {
  state: UpdateView;
  onCheck: () => void;
  onDownload: () => void;
  onInstall: () => void;
}) {
  const busy = state.phase === "checking" || state.phase === "downloading";
  const status =
    state.phase === "checking"
      ? "检查中…"
      : state.phase === "current"
        ? state.error
          ? "检查失败，请检查网络后重试"
          : "已是最新版本"
        : state.phase === "available"
          ? `发现新版本 ${state.version}`
          : state.phase === "downloading"
            ? `下载中 ${state.progress ?? 0}%`
            : state.phase === "ready"
              ? `已下载 ${state.version}，点「更新并重启」安装`
              : state.phase === "error"
                ? "更新失败，请检查网络后重试"
                : "";
  const showNotes =
    (state.phase === "available" || state.phase === "ready") && state.notes.trim() !== "";

  return (
    <Card title="更新">
      <Row label="上次检查">
        <span className="sub">
          {state.checkedAt ? formatCheckedAt(state.checkedAt) : "尚未检查"}
        </span>
      </Row>
      {status && (
        <Row label="状态">
          <span
            className={`sub update-state ${state.phase}`}
            title={state.error ?? undefined}
          >
            {status}
          </span>
        </Row>
      )}
      <div className="update-actions">
        <button type="button" className="btn" disabled={busy} onClick={onCheck}>
          检查更新
        </button>
        {state.phase === "available" && (
          <button type="button" className="btn primary" onClick={onDownload}>
            下载 {state.version}
          </button>
        )}
        {state.phase === "ready" && (
          <button type="button" className="btn primary" onClick={onInstall}>
            更新并重启
          </button>
        )}
      </div>
      {state.phase === "downloading" && (
        <div className="update-bar">
          <i style={{ width: `${state.progress ?? 0}%` }} />
        </div>
      )}
      {showNotes && (
        <div className="release-notes">
          <div className="release-notes-head">
            更新内容{state.date ? ` · ${state.date}` : ""}
          </div>
          <pre>{state.notes}</pre>
        </div>
      )}
    </Card>
  );
}
const PHASE_LABEL: Record<string, string> = {
  idle: "空闲",
  armed: "待触发（光标已在触发区）",
  disarmed: "已触发，等待离开触发区",
};

const GUARD_LABEL: Record<string, string> = {
  "": "无",
  locked: "锁屏",
  button: "按着鼠标键",

  taskbar: "光标在任务栏上",
  blacklist: "前景应用在黑名单",
  paused: "已暂停",
  disabled: "总开关关闭",
};

const LINKS = [
  { label: "GitHub 项目", url: "https://github.com/punwong/hexbench", icon: <Github size={15} /> },
  {
    label: "问题反馈",
    url: "https://github.com/punwong/hexbench/issues",
    icon: <Inbox size={15} />,
  },
  {
    label: "更新日志",
    url: "https://github.com/punwong/hexbench/blob/main/docs/CHANGELOG.md",
    icon: <ScrollText size={15} />,
  },
  {
    label: "MIT 许可证",
    url: "https://github.com/punwong/hexbench/blob/main/LICENSE",
    icon: <Scale size={15} />,
  },
] as const;

export function AboutPage({
  snap,
  updateState,
  onCheck,
  onDownload,
  onInstall,
}: {
  snap: Snapshot;
  updateState: UpdateView;
  onCheck: () => void;
  onDownload: () => void;
  onInstall: () => void;
}) {
  const [copyState, setCopyState] = useState("");

  return (
    <div className="stack">
      <div className="about-brand">
        <span className="about-mark" aria-hidden="true" />
        <div>
          <div className="about-name">HexBench</div>
          <div className="sub">版本 v{snap.version}</div>
        </div>
      </div>

      <UpdateCard
        state={updateState}
        onCheck={onCheck}
        onDownload={onDownload}
        onInstall={onInstall}
      />

      <Card title="链接">
        <div className="link-list">
          {LINKS.map((link) => (
            <button
              type="button"
              key={link.url}
              className="link-row"
              onClick={() => void openUrl(link.url)}
            >
              {link.icon}
              <span>{link.label}</span>
              <ExternalLink size={13} className="link-glyph" />
            </button>
          ))}
        </div>
        <div className="footnote">
          项目名与图标不在 MIT 授权范围内；衍生分发请改名换图标。
        </div>
      </Card>

      <Card title="反馈">
        <div className="sub" style={{ marginBottom: 4 }}>
          遇到问题时，附上诊断信息能更快定位。
        </div>
        <details className="adv-details">
          <summary>诊断信息</summary>
          <dl className="kv">
            <dt>引擎状态</dt>
            <dd>{PHASE_LABEL[snap.enginePhase] ?? snap.enginePhase}</dd>
            <dt>当前命中</dt>
            <dd>{snap.hit ? `${snap.hit.side === "left" ? "左" : "右"}边缘 · 第 ${snap.hit.monitor + 1} 块屏` : "无"}</dd>
            <dt>防误触拦截</dt>
            <dd>{GUARD_LABEL[snap.guard ?? ""] ?? (snap.guard ?? "无")}</dd>
            <dt>轮询次数</dt>
            <dd>{snap.stats.ticks}</dd>
            <dt>触发次数</dt>
            <dd>{snap.stats.fires}</dd>
            <dt>发送失败</dt>
            <dd>{snap.stats.failures}</dd>
          </dl>
          <div className="adv-actions">
            <button
              type="button"
              className="btn"
              onClick={() => {
                void (async () => {
                  try {
                    await copyText(await diagnostics());
                    setCopyState("诊断信息已复制");
                  } catch (reason) {
                    setCopyState(`复制失败：${String(reason)}`);
                  }
                })();
              }}
            >
              复制诊断信息
            </button>
            {copyState && <span className="sub">{copyState}</span>}
          </div>
        </details>
      </Card>
    </div>
  );
}

/** 热键按工具分组：每个工具一组，工具页里不再重复配置。 */
const HOTKEY_GROUPS: Array<{
  tool: string;
  rows: Array<{ key: "pause" | "launcher"; label: string }>;
}> = [
  {
    tool: "快捷面板",
    rows: [{ key: "launcher", label: "呼出面板" }],
  },
  {
    tool: "边缘桌面",
    rows: [{ key: "pause", label: "暂停 / 继续" }],
  },
];

function comboFrom(event: KeyboardEvent): string | null {
  if (["Control", "Alt", "Shift", "Meta"].includes(event.key)) return null;
  const parts: string[] = [];
  if (event.ctrlKey) parts.push("Ctrl");
  if (event.altKey) parts.push("Alt");
  if (event.shiftKey) parts.push("Shift");
  if (event.metaKey) parts.push("Super");
  if (parts.length === 0) return null;
  let key = event.key;
  if (key === " ") key = "Space";
  else if (key.length === 1) key = key.toUpperCase();
  parts.push(key);
  return parts.join("+");
}

function HotkeyCapture({ onCapture }: { onCapture: (combo: string) => void }) {
  const [listening, setListening] = useState(false);

  useEffect(() => {
    if (!listening) return;
    const handler = (event: KeyboardEvent) => {
      event.preventDefault();
      event.stopPropagation();
      if (event.key === "Escape") {
        setListening(false);
        return;
      }
      const combo = comboFrom(event);
      if (combo) {
        onCapture(combo);
        setListening(false);
      }
    };
    window.addEventListener("keydown", handler, true);
    return () => window.removeEventListener("keydown", handler, true);
  }, [listening, onCapture]);

  return (
    <button
      type="button"
      className="btn"
      onClick={() => setListening(true)}
      // 点别处就去听别的控件了，别一直挂着「按下组合键…」。
      onBlur={() => setListening(false)}
      aria-label="设置热键"
    >
      {listening ? "按下组合键…" : "设置热键"}
    </button>
  );
}


function formatBytes(size: number): string {
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(0)} KB`;
  if (size < 1024 * 1024 * 1024) return `${(size / 1024 / 1024).toFixed(1)} MB`;
  return `${(size / 1024 / 1024 / 1024).toFixed(1)} GB`;
}

export function HotkeysPage({
  config,
  update,
  snap,
}: {
  config: Config;
  update: UpdateConfig;
  snap: Snapshot;
}) {
  return (
    <div className="stack">
      {HOTKEY_GROUPS.map((group) => (
        <Card key={group.tool} title={group.tool}>
          {group.rows.map((row) => (
            <Row key={row.key} label={row.label}>
              <div className="hotkey-cell">
                <code>{config.hotkeys[row.key] || "未设置"}</code>
                <HotkeyCapture
                  onCapture={(combo) => update({ hotkeys: { [row.key]: combo } })}
                />
                <button
                  type="button"
                  className="btn"
                  onClick={() => update({ hotkeys: { [row.key]: "" } })}
                >
                  清除
                </button>
              </div>
            </Row>
          ))}
        </Card>
      ))}

      {snap.hotkeyError && (
        <p className="sub" style={{ color: "#b42318", margin: 0 }}>
          {snap.hotkeyError}
        </p>
      )}
    </div>
  );
}

/**
 * 快捷面板：一个键呼出的面板。设置分成三块，对应三个标签：
 *   分类 —— 根目录下的每个子文件夹就是一个分类；
 *   固定文件夹 —— 另外钉住的几个普通文件夹；
 *   内置网页 —— 网页窗口顶栏上的那些站点。
 */
/// 可拖动排序的行。自己处理鼠标事件，不用 HTML5 拖放：
/// 无边框窗口里更稳，也不依赖系统拖放循环。
function SortableRows({
  items,
  onReorder,
  trailing,
}: {
  items: Array<{ key: string; label: string; hint?: string }>;
  onReorder: (from: number, to: number) => void;
  trailing?: (index: number) => ReactNode;
}) {
  const listRef = useRef<HTMLDivElement>(null);
  const [dragging, setDragging] = useState<number | null>(null);
  const [over, setOver] = useState<number | null>(null);
  // 监听器里读不到最新状态，用 ref 兜住。
  const stateRef = useRef<{ from: number | null; to: number | null }>({ from: null, to: null });

  /** 指针纵向落在第几行；落在最后一行下方就算最后一行。 */
  const rowAt = (y: number): number | null => {
    const rows = listRef.current?.querySelectorAll<HTMLElement>(".sortable-row");
    if (!rows || rows.length === 0) return null;
    for (let index = 0; index < rows.length; index += 1) {
      const rect = rows[index].getBoundingClientRect();
      if (y >= rect.top && y <= rect.bottom) return index;
    }
    const last = rows[rows.length - 1].getBoundingClientRect();
    return y > last.bottom ? rows.length - 1 : 0;
  };

  useEffect(() => {
    if (dragging === null) return;

    const onMove = (event: MouseEvent) => {
      const next = rowAt(event.clientY);
      if (next !== null && next !== stateRef.current.to) {
        stateRef.current.to = next;
        setOver(next);
      }
    };
    const onUp = () => {
      const { from, to } = stateRef.current;
      if (from !== null && to !== null && from !== to) onReorder(from, to);
      stateRef.current = { from: null, to: null };
      setDragging(null);
      setOver(null);
    };

    // 把行拖出窗口再松手，mouseup 收不到：窗口一失焦就当作取消，
    // 否则这一行会一直挂着拖动高亮，下一次点击还会带着旧位置误排序。
    const onBlur = () => {
      stateRef.current = { from: null, to: null };
      setDragging(null);
      setOver(null);
    };

    document.addEventListener("mousemove", onMove);
    document.addEventListener("mouseup", onUp);
    window.addEventListener("blur", onBlur);
    return () => {
      document.removeEventListener("mousemove", onMove);
      document.removeEventListener("mouseup", onUp);
      window.removeEventListener("blur", onBlur);
    };
  }, [dragging, onReorder]);

  return (
    <div className="sortable" ref={listRef}>
      {items.map((item, index) => (
        <div
          key={item.key}
          className={`sortable-row${dragging === index ? " dragging" : ""}${
            over === index && dragging !== null && dragging !== index ? " over" : ""
          }`}
          onMouseDown={(event) => {
            if (event.button !== 0) return;
            // 行尾的按钮要能正常点，别被拖拽抢走。
            if ((event.target as HTMLElement).closest("button")) return;
            event.preventDefault();
            stateRef.current = { from: index, to: index };
            setDragging(index);
            setOver(index);
          }}
        >
          <span className="grip" aria-hidden="true">
            ⠿
          </span>
          <span className="sortable-label">{item.label}</span>
          {item.hint && <span className="sub">{item.hint}</span>}
          <span className="sortable-actions">{trailing?.(index)}</span>
        </div>
      ))}
    </div>
  );
}
/** 眼睛按钮：亮 = 显示，灰 = 隐藏。 */
function EyeButton({ hidden, onClick }: { hidden: boolean; onClick: () => void }) {
  return (
    <button
      type="button"
      className={`icon-btn${hidden ? " off" : ""}`}
      title={hidden ? "在面板里隐藏：点一下显示" : "在面板里显示：点一下隐藏"}
      onClick={onClick}
    >
      {hidden ? <EyeOff size={16} /> : <Eye size={16} />}
    </button>
  );
}

function moveItem<T>(list: T[], from: number, to: number): T[] {
  const next = [...list];
  const [moved] = next.splice(from, 1);
  next.splice(to, 0, moved);
  return next;
}

function folderLabel(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const index = Math.max(trimmed.lastIndexOf("\\"), trimmed.lastIndexOf("/"));
  return index >= 0 ? trimmed.slice(index + 1) : trimmed;
}

export function LauncherPage({
  config,
  update,
  tab,
  onTabChange,
}: {
  config: Config;
  update: UpdateConfig;
  tab: "items" | "folders" | "sites" | "notes";
  onTabChange: (tab: "items" | "folders" | "sites" | "notes") => void;
}) {
  const root = config.launcher.root;
  const webOn = config.launcher.webEnabled !== false;
  const folders = config.launcher.folders ?? [];
  const hidden = config.launcher.hiddenCategories ?? [];
  const [categories, setCategories] = useState<LauncherCategoryInfo[]>([]);
  // 「清空」不可撤销：根目录和固定文件夹各问一句。
  const [askClear, setAskClear] = useState<null | "root" | "folders">(null);

  // 根目录变了就重新读一遍分类（后端已经按配置顺序排好）。
  useEffect(() => {
    let alive = true;
    void launcherCategoryList(root)
      .then((list) => alive && setCategories(list))
      .catch(() => alive && setCategories([]));
    return () => {
      alive = false;
    };
  }, [root]);

  const chooseRoot = async () => {
    const picked = await pickFolder();
    if (picked) update({ launcher: { root: picked } });
  };

  /** 显示 / 隐藏某一项（分类名，或 __root__）。 */
  const toggleHidden = (name: string) => {
    const next = hidden.includes(name)
      ? hidden.filter((entry) => entry !== name)
      : [...hidden, name];
    update({ launcher: { hiddenCategories: next } });
  };

  const addFolder = async () => {
    const picked = await pickFolder();
    if (!picked || folders.includes(picked)) return;
    update({ launcher: { folders: [...folders, picked] } });
  };

  // ---- 收藏 ----
  const [confirmClear, setConfirmClear] = useState(false);
  const [clearing, setClearing] = useState(false);
  const [notesNote, setNotesNote] = useState<string | null>(null);

  // 配置留空时实际用的是默认目录。别在 folder 变化后回读后端解析的路径：
  // 保存有 300 ms 防抖，回读常赶在落盘前，改完目录这一栏还显示旧的。自定义
  // 目录直接显示配置值，只有留空时才需要这个与配置无关的默认目录。
  const [defaultNotesPath, setDefaultNotesPath] = useState("");
  useEffect(() => {
    let alive = true;
    void notesDefaultFolder()
      .then((path) => {
        if (alive) setDefaultNotesPath(path);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, []);
  const notesPath = config.notes.folder || defaultNotesPath;

  const chooseNotesFolder = async () => {
    const picked = await pickFolder();
    if (picked) update({ notes: { folder: picked } });
  };

  const clearNotes = async () => {
    setConfirmClear(false);
    setClearing(true);
    try {
      const count = await notesClearAll();
      setNotesNote(count ? `已把 ${count} 条便签移到回收站` : "没有可清空的便签");
    } catch (error) {
      setNotesNote(String(error));
    } finally {
      setClearing(false);
    }
  };

  return (
    <div className="stack">
      <Card>
        <Row label="启用快捷面板" hint="关掉后热键和面板都不再生效">
          <Toggle
            checked={config.launcher.enabled}
            onChange={(value) => update({ launcher: { enabled: value } })}
          />
        </Row>
      </Card>
      <Tabs
        value={tab}
        onChange={(value) =>
          onTabChange(value as "items" | "folders" | "sites" | "notes")
        }
        options={[
          { value: "items", label: "分类" },
          { value: "folders", label: "固定文件夹" },
          { value: "sites", label: "内置网页" },
          { value: "notes", label: "便签" },
        ]}
      />

      {tab === "items" ? (
        <Card>
          <Row label="根目录" hint="每个子文件夹就是一个分类">
            <div className="hotkey-cell">
              <code>{root || "未选择"}</code>
              <button type="button" className="btn" onClick={() => void chooseRoot()}>
                选择
              </button>
              <button
                type="button"
                className="btn"
                disabled={!root}
                onClick={() => setAskClear("root")}
              >
                清空…
              </button>
            </div>
          </Row>

          <Row label="呼出位置">
            <div className="hotkey-cell">
              <Toggle
                checked={config.launcher.followCursor}
                onChange={(value) => update({ launcher: { followCursor: value } })}
              />
              <span style={{ fontSize: 12.5, color: "var(--muted)" }}>
                {config.launcher.followCursor ? "贴着鼠标" : "屏幕居中"}
              </span>
            </div>
          </Row>

          <Row label="面板宽度" hint="占工作区宽度">
            <Slider
              value={config.launcher.widthPercent}
              min={25}
              max={80}
              step={1}
              onChange={(value) => update({ launcher: { widthPercent: value } })}
              format={(value) => `${value}%`}
            />
          </Row>
          <Row label="面板高度" hint="占工作区高度">
            <Slider
              value={config.launcher.heightPercent}
              min={30}
              max={80}
              step={1}
              onChange={(value) => update({ launcher: { heightPercent: value } })}
              format={(value) => `${value}%`}
            />
          </Row>
          <Row label="记住调整后的面板大小">
            <Toggle
              checked={config.launcher.panelRememberSize ?? true}
              onChange={(next) => update({ launcher: { panelRememberSize: next } })}
            />
          </Row>
          <Row label="记住浏览位置" hint="下次呼出回到上次停的文件夹">
            <Toggle
              checked={config.launcher.rememberLocation}
              onChange={(value) => update({ launcher: { rememberLocation: value } })}
            />
          </Row>

          {root && (
            <div style={{ marginTop: 16 }}>
              <div className="row-hint" style={{ marginBottom: 8 }}>
                拖动排序 · 点眼睛图标隐藏分类
              </div>

              {/* 「全部」：固定在第一条，只能显示 / 隐藏 */}
              <div className="sortable-row" style={{ cursor: "default" }}>
                <span className="grip" style={{ opacity: 0.25, cursor: "default" }}>
                  ⠿
                </span>
                <span className="sortable-label">全部</span>
                <span className="sortable-actions">
                  <EyeButton
                    hidden={hidden.includes("__all__")}
                    onClick={() => toggleHidden("__all__")}
                  />
                </span>
              </div>

              <div style={{ height: 6 }} />

              {categories.length === 0 ? (
                <EmptyState
                  title="还没有分类"
                  hint="在根目录里建一个文件夹。"
                />
              ) : (
                <SortableRows
                  items={categories.map((category) => ({
                    key: category.path,
                    label: category.name,
                    hint: `${category.count} 项`,
                  }))}
                  onReorder={(from, to) => {
                    const next = moveItem(categories, from, to);
                    setCategories(next);
                    update({
                      launcher: { categoryOrder: next.map((category) => category.name) },
                    });
                  }}
                  trailing={(index) => (
                    <EyeButton
                      // 用配置里的 hidden 列表，不用分类快照：快照只在根目录变化时
                      // 重读，点了眼睛之后它还是旧值，图标会一直不更新。
                      hidden={hidden.includes(categories[index].name)}
                      onClick={() => toggleHidden(categories[index].name)}
                    />
                  )}
                />
              )}
            </div>
          )}
        </Card>
      ) : tab === "sites" ? (
        <Card>
          <Row label="启用网页窗口">
            <Toggle
              checked={webOn}
              onChange={(value) => update({ launcher: { webEnabled: value } })}
            />
          </Row>

          {/* 关掉网页窗口后，下面这些尺寸 / 跟随选项都不生效：整块置灰，
              别让它们看起来还能调。 */}
          <fieldset className="fieldset" disabled={!webOn}>
            {!webOn && (
              <p className="sub fieldset-note">启用网页窗口后，这些选项才会生效。</p>
            )}
          <Row label="网页窗口宽度" hint="占工作区宽度">
            <Slider
              value={config.launcher.siteWidthPercent ?? 24}
              min={10}
              max={90}
              step={1}
              onChange={(value) => update({ launcher: { siteWidthPercent: value } })}
              format={(value) => `${value}%`}
            />
          </Row>
          <Row label="网页窗口高度" hint="占工作区高度，可以比面板高">
            <Slider
              value={config.launcher.siteHeightPercent ?? 85}
              min={20}
              max={100}
              step={1}
              onChange={(value) => update({ launcher: { siteHeightPercent: value } })}
              format={(value) => `${value}%`}
            />
          </Row>

          <Row label="跟着呼出面板走">
            <Toggle
              checked={config.launcher.siteFollowPanel ?? false}
              onChange={(value) => update({ launcher: { siteFollowPanel: value } })}
            />
          </Row>

          <Row label="记住位置和大小" hint="关掉则每次按上面的宽高重新摆">
            <Toggle
              checked={config.launcher.siteRememberPosition ?? true}
              onChange={(value) => update({ launcher: { siteRememberPosition: value } })}
            />
          </Row>

          {(config.launcher.siteFollowPanel ?? false) ? null : (
            <Row label="固定在屏幕哪一侧">
              <Segmented
                value={config.launcher.siteSide ?? "right"}
                onChange={(value) => update({ launcher: { siteSide: value } })}
                options={[
                  { value: "left", label: "左侧" },
                  { value: "right", label: "右侧" },
                ]}
              />
            </Row>
          )}

          <Row label="收起后保留" hint="面板收起后网页还开多久，0 = 立刻关闭">
            <Slider
              value={config.launcher.siteKeepAliveSecs ?? 90}
              min={0}
              max={300}
              step={10}
              onChange={(value) => update({ launcher: { siteKeepAliveSecs: value } })}
              format={(value) => (value === 0 ? "立刻关闭" : `${value} 秒`)}
            />
          </Row>

          {(config.launcher.sites ?? []).length === 0 && (
            <EmptyState
              title="还没有网址"
              hint="添加后从面板里直接打开。"
            />
          )}

          {(config.launcher.sites ?? []).map((entry, index) => (
            <div key={index} className="sortable-row" style={{ cursor: "default" }}>
              <input
                className="side-input"
                style={{ width: 110 }}
                value={entry.name}
                placeholder="名称"
                onChange={(event) =>
                  update({
                    launcher: {
                      sites: (config.launcher.sites ?? []).map((item, i) =>
                        i === index ? { ...item, name: event.target.value } : item,
                      ),
                    },
                  })
                }
              />
              <input
                className="side-input"
                style={{ flex: 1 }}
                value={entry.url}
                placeholder="https://chatgpt.com"
                onChange={(event) =>
                  update({
                    launcher: {
                      sites: (config.launcher.sites ?? []).map((item, i) =>
                        i === index ? { ...item, url: event.target.value } : item,
                      ),
                    },
                  })
                }
                onBlur={() => {
                  const raw = entry.url.trim();
                  if (!raw) return;
                  // 只打 `bing.com` 也认：补上协议再存，省得后面报「网址不对」。
                  const normalized = raw.includes("://") ? raw : `https://${raw}`;
                  const patched = (config.launcher.sites ?? []).map((item, i) =>
                    i === index ? { ...item, url: normalized } : item,
                  );
                  if (normalized !== raw) update({ launcher: { sites: patched } });
                  // 名字空着就顺手抓一下标题；抓到之后照样能改。
                  if (entry.name.trim()) return;
                  void siteTitle(normalized).then((title) => {
                    if (!title) return;
                    // 按地址找这一行，别拿发请求时的旧数组整份写回去：
                    // 抓标题这一两秒里，别的行可能已经增删、改名、换顺序。
                    update((current) => {
                      const sites = current.launcher.sites ?? [];
                      const target = sites.findIndex(
                        (item) => item.url === normalized && !item.name.trim(),
                      );
                      if (target < 0) return null;
                      return {
                        launcher: {
                          sites: sites.map((item, i) =>
                            i === target ? { ...item, name: title } : item,
                          ),
                        },
                      };
                    });
                  });
                }}
              />
              <span className="sortable-actions">
                <button
                  type="button"
                  className="btn"
                  disabled={index === 0}
                  title="上移"
                  onClick={() =>
                    update({
                      launcher: { sites: moveItem(config.launcher.sites ?? [], index, index - 1) },
                    })
                  }
                >
                  ↑
                </button>
                <button
                  type="button"
                  className="btn"
                  disabled={index === (config.launcher.sites ?? []).length - 1}
                  title="下移"
                  onClick={() =>
                    update({
                      launcher: { sites: moveItem(config.launcher.sites ?? [], index, index + 1) },
                    })
                  }
                >
                  ↓
                </button>
                <button
                  type="button"
                  className="btn"
                  onClick={() =>
                    update({
                      launcher: {
                        sites: (config.launcher.sites ?? []).filter((_, i) => i !== index),
                      },
                    })
                  }
                >
                  移除
                </button>
              </span>
            </div>
          ))}

          <div style={{ marginTop: 12 }}>
            <button
              type="button"
              className="btn"
              onClick={() =>
                update({
                  launcher: { sites: [...(config.launcher.sites ?? []), { name: "", url: "" }] },
                })
              }
            >
              添加网址
            </button>
          </div>
          </fieldset>
        </Card>
      ) : tab === "notes" ? (
        <Card>
          <Row label="启用便签" hint="在面板左侧显示">
            <Toggle
              checked={config.notes.enabled !== false}
              onChange={(value) => update({ notes: { enabled: value } })}
            />
          </Row>
          <Row
            label="保存位置"
            hint="默认在文档下，也可以放到同步盘"
          >
            <div className="hotkey-cell">
              <code title={notesPath}>{notesPath || "文档\\HexBench 收藏"}</code>
              <button
                type="button"
                className="btn"
                onClick={() => void chooseNotesFolder()}
              >
                选择
              </button>
              <button
                type="button"
                className="btn"
                onClick={() =>
                  void notesOpenFolder().catch((error) => setNotesNote(String(error)))
                }
              >
                打开
              </button>
              <button
                type="button"
                className="btn"
                disabled={!config.notes.folder}
                title="改回默认目录"
                onClick={() => update({ notes: { folder: "" } })}
              >
                默认
              </button>
            </div>
          </Row>
          <Row label="发送后自动回车" hint="适合聊天框、搜索框；往邮件正文里发时别开">
            <Toggle
              checked={config.notes.sendEnter}
              onChange={(value) => update({ notes: { sendEnter: value } })}
            />
          </Row>
          <Row label="清空全部便签" hint="便签和图片都会进回收站，之后还能从回收站找回">
            <button
              type="button"
              className="btn"
              disabled={clearing}
              onClick={() => setConfirmClear(true)}
            >
              {clearing ? "清空中…" : "清空…"}
            </button>
          </Row>
          {notesNote && <div className="row-hint">{notesNote}</div>}
          <ConfirmDialog
            open={confirmClear}
            title="清空全部便签？"
            confirmLabel="清空"
            onCancel={() => setConfirmClear(false)}
            onConfirm={() => void clearNotes()}
          >
            便签和图片都会进回收站，之后还能从回收站找回。
          </ConfirmDialog>
        </Card>
      ) : (
        <Card>
          {folders.length === 0 ? (
            <EmptyState
              title="还没有固定文件夹"
              hint="常用的文件夹会固定显示在面板里。"
            />
          ) : (
            <SortableRows
              items={folders.map((folder) => ({
                key: folder,
                label: folderLabel(folder),
                hint: folder,
              }))}
              onReorder={(from, to) =>
                update({ launcher: { folders: moveItem(folders, from, to) } })
              }
              trailing={(index) => (
                <button
                  type="button"
                  className="btn"
                  onClick={() =>
                    update({ launcher: { folders: folders.filter((_, i) => i !== index) } })
                  }
                >
                  移除
                </button>
              )}
            />
          )}

          <div style={{ marginTop: 12, display: "flex", gap: 8 }}>
            <button type="button" className="btn" onClick={() => void addFolder()}>
              添加文件夹
            </button>
            {folders.length > 0 && (
              <button
                type="button"
                className="btn"
                onClick={() => setAskClear("folders")}
              >
                全部清空…
              </button>
            )}
          </div>
        </Card>
      )}
      <ConfirmDialog
        open={askClear !== null}
        title={askClear === "root" ? "清空根目录？" : "清空全部固定文件夹？"}
        confirmLabel="清空"
        onCancel={() => setAskClear(null)}
        onConfirm={() => {
          const mode = askClear;
          setAskClear(null);
          if (mode === "root") update({ launcher: { root: "", categoryOrder: [] } });
          if (mode === "folders") update({ launcher: { folders: [] } });
        }}
      >
        {askClear === "root"
          ? "面板不再列出分类。文件夹本身不会被删除，重新选择根目录就能恢复。"
          : "这些文件夹会从面板里移除，文件夹本身不会被删除，可以再添加回来。"}
      </ConfirmDialog>
    </div>
  );
}
/// 整理规则的编辑器：文字框失焦才保存，开关 / 增删立刻保存。
/// 后端会给新规则补 id，回来的是同一份内容，所以直接按规则数组整份提交。
function FenceRulesEditor({ fence }: { fence: FenceSettings }) {
  const [draft, setDraft] = useState<FenceRule[]>(fence.rules ?? []);
  const [known, setKnown] = useState<KnownFolders | null>(null);
  const [preview, setPreview] = useState<{ total: number; names: string[] } | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // 上一次和后端对齐过的内容：别的设置触发的 config 刷新不该把正在敲的字冲掉。
  const savedRef = useRef(JSON.stringify(fence.rules ?? []));

  useEffect(() => {
    const incoming = JSON.stringify(fence.rules ?? []);
    if (incoming !== savedRef.current) {
      savedRef.current = incoming;
      setDraft(fence.rules ?? []);
    }
  }, [fence.rules]);

  useEffect(() => {
    void fenceKnownFolders().then(setKnown).catch(() => undefined);
  }, []);

  const save = (next: FenceRule[]) => {
    setDraft(next);
    setNote(null);
    // 这份就是接下来会从 config 回来的内容：先记下，保存期间别的设置触发
    // 刷新时就不会把用户正在敲的字换回旧的。
    savedRef.current = JSON.stringify(next);
    void fenceUpdate(fence.id, { rules: next }).catch((problem) => setNote(String(problem)));
  };

  const patch = (index: number, change: Partial<FenceRule>) => {
    setDraft((current) =>
      current.map((rule, at) => (at === index ? { ...rule, ...change } : rule)),
    );
  };

  const without = (index: number) => draft.filter((_, at) => at !== index);

  const addSource = (index: number, source: string | null | undefined) => {
    const rule = draft[index];
    if (!source || !rule || rule.sources.includes(source)) return;
    save(
      draft.map((item, at) =>
        at === index ? { ...item, sources: [...item.sources, source] } : item,
      ),
    );
  };

  const pickSource = async (index: number) => {
    const picked = await pickFolder();
    addSource(index, picked);
  };

  const addRule = () => {
    const rule: FenceRule = {
      id: `r${Date.now().toString(36)}`,
      name: "新规则",
      enabled: true,
      exts: [],
      nameContains: "",
      minMb: 0,
      maxMb: 0,
      olderThanDays: 0,
      // 新规则先不挂来源：一条没有任何条件的规则配上下载目录，
      // 下一次下载就会把整个目录扫进来。来源由用户明确加。
      sources: [],
      subdir: "",
    };
    save([...draft, rule]);
  };

  const runPreview = async () => {
    setBusy(true);
    setNote(null);
    try {
      const matches = await fenceRulePreview(fence.id);
      setPreview({
        total: matches.length,
        names: matches.slice(0, 8).map((match) => match.name),
      });
    } catch (problem) {
      setNote(String(problem));
    } finally {
      setBusy(false);
    }
  };

  const runNow = async () => {
    setBusy(true);
    setNote(null);
    try {
      const result = await fenceRuleRun(fence.id);
      setPreview(null);
      setNote(
        result.moved > 0
          ? `已收进 ${result.moved} 项（匹配 ${result.matched} 项）`
          : "没有可整理的文件",
      );
    } catch (problem) {
      setNote(String(problem));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="rule-editor">
      <p className="sub" style={{ marginTop: 0 }}>
        来源目录的文件按规则收进盒子；每次整理算一步撤销（Ctrl+Z）。
      </p>
      <Row label="自动整理" hint="来源目录一有变化就按规则收；关掉只能手动点「立即整理」">
        <Toggle
          checked={fence.rulesAuto}
          onChange={(value) => void fenceUpdate(fence.id, { rulesAuto: value })}
        />
      </Row>
      {draft.length === 0 && (
        <div className="row-hint">
          还没有规则，现在就是普通收纳盒。
        </div>
      )}
      {draft.map((rule, index) => (
        <div className="rule-card" key={rule.id || index}>
          <div className="rule-head">
            <Toggle
              checked={rule.enabled}
              onChange={(value) =>
                save(draft.map((item, at) => (at === index ? { ...item, enabled: value } : item)))
              }
            />
            <input
              className="side-input"
              style={{ flex: 1 }}
              value={rule.name}
              placeholder="规则名（也是默认子目录名）"
              onChange={(event) => patch(index, { name: event.target.value })}
              onBlur={() => save(draft)}
            />
            <button
              type="button"
              className="icon-btn danger"
              title="删除这条规则"
              onClick={() => save(without(index))}
            >
              <Trash2 size={14} />
            </button>
          </div>
          <div className="rule-grid">
            <label>
              后缀
              <input
                className="side-input"
                value={rule.exts.join(", ")}
                placeholder="png, jpg（空 = 不限）"
                onChange={(event) =>
                  patch(index, {
                    exts: event.target.value
                      .split(/[,，\s]+/)
                      .map((ext) => ext.trim())
                      .filter(Boolean),
                  })
                }
                onBlur={() => save(draft)}
              />
            </label>
            <label>
              名称包含
              <input
                className="side-input"
                value={rule.nameContains}
                placeholder="可选"
                onChange={(event) => patch(index, { nameContains: event.target.value })}
                onBlur={() => save(draft)}
              />
            </label>
            <label>
              大于（MB）
              <input
                className="side-input"
                type="number"
                min={0}
                value={rule.minMb}
                onChange={(event) => patch(index, { minMb: Number(event.target.value) || 0 })}
                onBlur={() => save(draft)}
              />
            </label>
            <label>
              小于（MB）
              <input
                className="side-input"
                type="number"
                min={0}
                value={rule.maxMb}
                onChange={(event) => patch(index, { maxMb: Number(event.target.value) || 0 })}
                onBlur={() => save(draft)}
              />
            </label>
            <label>
              早于（天）
              <input
                className="side-input"
                type="number"
                min={0}
                value={rule.olderThanDays}
                onChange={(event) =>
                  patch(index, { olderThanDays: Number(event.target.value) || 0 })
                }
                onBlur={() => save(draft)}
              />
            </label>
            <label>
              收进子目录
              <input
                className="side-input"
                value={rule.subdir}
                placeholder="可选，例如 图片"
                onChange={(event) => patch(index, { subdir: event.target.value })}
                onBlur={() => save(draft)}
              />
            </label>
          </div>
          <div className="rule-sources">
            <span className="row-label">来源</span>
            {rule.sources.length === 0 && (
              <span className="row-hint">还没有来源，这条规则不会收任何文件</span>
            )}
            {rule.sources.map((source) => (
              <span className="chip" key={source} title={source}>
                <span>{source}</span>
                <button
                  type="button"
                  title="移除这个来源"
                  onClick={() =>
                    save(
                      draft.map((item, at) =>
                        at === index
                          ? { ...item, sources: item.sources.filter((one) => one !== source) }
                          : item,
                      ),
                    )
                  }
                >
                  ×
                </button>
              </span>
            ))}
            {known?.desktop && !rule.sources.includes(known.desktop) && (
              <button type="button" className="btn" onClick={() => addSource(index, known.desktop)}>
                桌面
              </button>
            )}
            {known?.downloads && !rule.sources.includes(known.downloads) && (
              <button
                type="button"
                className="btn"
                onClick={() => addSource(index, known.downloads)}
              >
                下载
              </button>
            )}
            <button type="button" className="btn" onClick={() => void pickSource(index)}>
              选文件夹…
            </button>
            {rule.sources.length > 0 &&
              rule.exts.length === 0 &&
              !rule.nameContains.trim() &&
              rule.minMb === 0 &&
              rule.maxMb === 0 &&
              rule.olderThanDays === 0 && (
                <span className="row-hint">
                  没有条件：这些来源里的所有文件都会被收进来
                </span>
              )}
          </div>
        </div>
      ))}
      <div className="rule-actions">
        <button type="button" className="btn" onClick={addRule}>
          <Plus size={14} style={{ marginRight: 4, verticalAlign: "-2px" }} />
          添加规则
        </button>
        <button
          type="button"
          className="btn"
          disabled={busy || draft.length === 0}
          onClick={() => void runPreview()}
        >
          预览
        </button>
        <button
          type="button"
          className="btn primary"
          disabled={busy || draft.length === 0}
          onClick={() => void runNow()}
        >
          立即整理
        </button>
      </div>
      {preview && (
        <div className="row-hint">
          匹配 {preview.total} 项
          {preview.total > 0
            ? `：${preview.names.join("、")}${preview.total > preview.names.length ? " …" : ""}`
            : ""}
        </div>
      )}
      {note && <div className="row-hint">{note}</div>}
    </div>
  );
}

/// 设置页里的「手势」：快捷面板文件区的右键滑动。
/// 桌面格子：总开关 + 格子列表。增删改走专属命令，其余设置走 save_config。
/** 格子摆在哪：屏幕用「主屏 / 屏 2」，尺寸换成像素，原始设备名只放 tooltip。 */
function fencePlacement(fence: FenceSettings, monitors: MonitorDto[]) {
  const monitor =
    monitors.find((item) => item.device === fence.monitor) ??
    monitors.find((item) => item.primary) ??
    null;
  const name = monitor ? (monitor.primary ? "主屏" : `屏 ${monitor.index + 1}`) : "屏幕";
  const pxW = monitor ? Math.round(monitor.workW * fence.w) : null;
  const pxH = monitor ? Math.round(monitor.workH * fence.h) : null;
  const corner = `${fence.x < 0.5 ? "左" : "右"}${fence.y < 0.5 ? "上" : "下"}`;
  const parts = [name];
  if (pxW && pxH) parts.push(`${pxW} × ${pxH}`);
  parts.push(corner);
  return {
    label: parts.join(" · "),
    title: `${fence.monitor || "默认屏幕"} · 宽 ${Math.round(fence.w * 100)}% × 高 ${Math.round(fence.h * 100)}%`,
  };
}

export function DesktopPage({
  config,
  update,
  monitors,
}: {
  config: Config;
  update: UpdateConfig;
  monitors: MonitorDto[];
}) {
  const desktop = config.desktop;
  const fences = desktop.fences ?? [];
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // 哪一格的整理规则展开着（同一时间只开一个）。
  const [openRules, setOpenRules] = useState<string | null>(null);
  // 删除收纳盒前问文件怎么处理；普通格子 / 空收纳盒直接确认删掉。
  const [deleting, setDeleting] = useState<{
    fence: FenceSettings;
    info: FenceBoxInfo;
  } | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<FenceSettings | null>(null);

  // 外观滑块要跟手：先改本地值，停手 220ms 再写配置、通知所有格子换色。
  const [opacity, setOpacity] = useState(desktop.opacity);
  const [frosted, setFrosted] = useState(desktop.frosted);
  const pendingAppearance = useRef<{ opacity: number; frosted: boolean } | null>(null);
  const appearanceTimer = useRef<number | undefined>(undefined);

  useEffect(() => {
    setOpacity(desktop.opacity);
    setFrosted(desktop.frosted);
  }, [desktop.opacity, desktop.frosted]);

  const applyAppearance = (nextOpacity: number, nextFrosted: boolean) => {
    // 设置页手里这份配置也立刻跟上：拖完滑块紧接着点别的开关时，保存的
    // 不会还是旧的不透明度。
    update({ desktop: { opacity: nextOpacity, frosted: nextFrosted } });
    pendingAppearance.current = { opacity: nextOpacity, frosted: nextFrosted };
    window.clearTimeout(appearanceTimer.current);
    appearanceTimer.current = window.setTimeout(() => {
      const pending = pendingAppearance.current;
      pendingAppearance.current = null;
      if (!pending) return;
      void fenceSetAppearance(pending.opacity, pending.frosted).catch((problem) =>
        setError(String(problem)),
      );
    }, 220);
  };

  // 页关掉时还有没落盘的改动：直接发出去，别让最后一格拖动白调。
  useEffect(
    () => () => {
      window.clearTimeout(appearanceTimer.current);
      const pending = pendingAppearance.current;
      if (pending) {
        void fenceSetAppearance(pending.opacity, pending.frosted).catch(() => undefined);
      }
    },
    [],
  );

  const addFence = async () => {
    const picked = await pickFolder();
    if (!picked) return;
    setBusy(true);
    setError(null);
    try {
      await fenceCreate(picked);
    } catch (problem) {
      setError(String(problem));
    } finally {
      setBusy(false);
    }
  };

  /// 新建收纳盒：幕后目录由后端建，不用选文件夹。
  const addBox = async () => {
    const used = fences.filter((fence) => fence.kind === "bucket").length;
    setBusy(true);
    setError(null);
    try {
      await fenceCreateBox(`收纳盒 ${used + 1}`);
    } catch (problem) {
      setError(String(problem));
    } finally {
      setBusy(false);
    }
  };

  /// 删除入口：收纳盒里有东西时先问文件怎么办，空的 / 普通格子直接删。
  const askDelete = async (fence: FenceSettings) => {
    setError(null);
    if (fence.kind === "bucket") {
      try {
        const info = await fenceBoxInfo(fence.id);
        if (info && info.items > 0) {
          setDeleting({ fence, info });
          return;
        }
      } catch (problem) {
        setError(String(problem));
        return;
      }
    }
    setConfirmDelete(fence);
  };

  const finishDelete = (mode: "release" | "purge" | "keep") => {
    const target = deleting;
    setDeleting(null);
    if (!target) return;
    void fenceDelete(target.fence.id, mode).catch((problem) => setError(String(problem)));
  };

  return (
    <div className="stack">
      <Card>
        <Row label="开启桌面格子" hint="关掉后桌面上的格子会消失，位置和设置会保留">
          <Toggle
            checked={desktop.enabled}
            onChange={(value) => update({ desktop: { enabled: value } })}
          />
        </Row>
        <Row label="按 Win+D 时保持可见">
          <Toggle
            checked={desktop.keepOnShowDesktop}
            onChange={(value) => update({ desktop: { keepOnShowDesktop: value } })}
          />
        </Row>
        <Row label="移动时吸附" hint="按住 Alt 拖动可临时暂停；对齐时显示参考线">
          <Toggle
            checked={desktop.snap}
            onChange={(value) => update({ desktop: { snap: value } })}
          />
        </Row>
        <Row label="吸附到 8px 网格" hint="不贴边时也按网格对齐">
          <Toggle
            checked={desktop.snapGrid}
            onChange={(value) => update({ desktop: { snapGrid: value } })}
          />
        </Row>
      </Card>

      <Card
        title="格子"
        action={
          <div style={{ display: "flex", gap: 8 }}>
            <button type="button" className="btn" disabled={busy || fences.length >= 8} onClick={() => void addFence()}>
              <FolderOpen size={14} style={{ marginRight: 4, verticalAlign: "-2px" }} />
              添加文件夹格子
            </button>
            <button
              type="button"
              className="btn primary"
              disabled={busy || fences.length >= 8}
              onClick={() => void addBox()}
            >
              <Inbox size={14} style={{ marginRight: 4, verticalAlign: "-2px" }} />
              新建收纳盒
            </button>
          </div>
        }
      >
        {fences.length === 0 ? (
          <EmptyState
            icon={<LayoutGrid size={22} />}
            title="还没有格子"
          />
        ) : (
          <div className="fence-list">
            {fences.map((fence) => {
              const isBox = fence.kind === "bucket";
              const ruleCount = fence.rules?.length ?? 0;
              const place = fencePlacement(fence, monitors);
              return (
                <div className="fence-item" key={fence.id}>
                  <div className="fence-row">
                    <span className="fence-glyph" aria-hidden>
                      {isBox ? <Inbox size={16} /> : <LayoutGrid size={16} />}
                    </span>
                    <span className="fence-text">
                      <span className="fence-name">
                        {fence.title || folderLabel(fence.path)}
                        {isBox && (
                          <span className="fence-badge">收纳盒</span>
                        )}
                        {fence.rolled ? "（卷起）" : ""}
                      </span>
                      <span className="fence-path mono" title={fence.path}>
                        {isBox ? `收纳目录 · ${fence.path}` : fence.path || "没有绑定文件夹"}
                      </span>
                      <span className="row-hint" title={place.title}>
                        {place.label}
                        {fence.zoom !== 100 ? ` · ${fence.zoom}% 缩放` : ""}
                        {fence.locked ? " · 位置已锁定" : ""}
                        {isBox && ruleCount > 0
                          ? ` · ${ruleCount} 条规则${fence.rulesAuto ? "（自动）" : "（手动）"}`
                          : ""}
                      </span>
                    </span>
                    <span className="fence-actions">
                      <button
                        type="button"
                        className="icon-btn"
                        title={fence.view === "list" ? "改成图标视图" : "改成列表视图"}
                        onClick={() =>
                          void fenceUpdate(fence.id, {
                            view: fence.view === "list" ? "grid" : "list",
                          }).catch((problem) => setError(String(problem)))
                        }
                      >
                        {fence.view === "list" ? <LayoutGrid size={16} /> : <List size={16} />}
                      </button>
                      <button
                        type="button"
                        className={`icon-btn${fence.locked ? " active" : ""}`}
                        title={fence.locked ? "解锁位置" : "锁定位置"}
                        onClick={() =>
                          void fenceUpdate(fence.id, { locked: !fence.locked }).catch((problem) =>
                            setError(String(problem)),
                          )
                        }
                      >
                        {fence.locked ? <Lock size={16} /> : <LockOpen size={16} />}
                      </button>
                      <EyeButton
                        hidden={!fence.visible}
                        onClick={() =>
                          void fenceUpdate(fence.id, { visible: !fence.visible }).catch(
                            (problem) => setError(String(problem)),
                          )
                        }
                      />
                      {isBox && (
                        <button
                          type="button"
                          className={`icon-btn${openRules === fence.id ? " active" : ""}`}
                          title="整理规则"
                          onClick={() =>
                            setOpenRules((current) => (current === fence.id ? null : fence.id))
                          }
                        >
                          <SlidersHorizontal size={16} />
                        </button>
                      )}
                      <button
                        type="button"
                        className="icon-btn"
                        title={isBox ? "打开收纳目录" : "在资源管理器中打开"}
                        disabled={!fence.path}
                        onClick={() => void openPath(fence.path)}
                      >
                        <FolderOpen size={16} />
                      </button>
                      <button
                        type="button"
                        className="icon-btn danger"
                        title="删除这个格子"
                        onClick={() => void askDelete(fence)}
                      >
                        <Trash2 size={16} />
                      </button>
                    </span>
                  </div>
                  {isBox && openRules === fence.id && <FenceRulesEditor fence={fence} />}
                </div>
              );
            })}
          </div>
        )}
        <div className="row-hint" style={{ marginTop: 10 }}>
          拖动标题栏移动，双击卷起，Alt+滚轮缩放。
        </div>
        {error && <div className="row-error">{error}</div>}
        {fences.length >= 8 && (
          <div className="row-hint" style={{ marginTop: 8 }}>
            最多 8 个格子。
          </div>
        )}
      </Card>

      <Card title="外观">
        <Row label="不透明度" hint="太低会影响阅读">
          <Slider
            value={opacity}
            min={30}
            max={100}
            step={1}
            onChange={(value) => {
              setOpacity(value);
              applyAppearance(value, frosted);
            }}
            format={(value) => `${value}%`}
          />
        </Row>
        <Row label="磨砂（模糊背景）">
          <Toggle
            checked={frosted}
            onChange={(value) => {
              setFrosted(value);
              applyAppearance(opacity, value);
            }}
          />
        </Row>
        <p className="sub" style={{ marginBottom: 0 }}>
          系统不支持磨砂时，格子会显示为纯色。
        </p>
      </Card>

      {deleting && (
        <div className="modal-backdrop" role="dialog" aria-modal="true" aria-label="删除收纳盒">
          <div className="modal">
            <h2>删除「{deleting.fence.title || "收纳盒"}」？</h2>
            <p>
              收纳盒里还有 {deleting.info.items} 个文件（{formatBytes(deleting.info.bytes)}）。
              文件怎么处理？
            </p>
            <div className="modal-actions" style={{ flexWrap: "wrap", justifyContent: "flex-end" }}>
              <button type="button" className="btn" onClick={() => setDeleting(null)}>
                取消
              </button>
              <button type="button" className="btn" onClick={() => finishDelete("keep")}>
                保留文件
              </button>
              <button type="button" className="btn danger" onClick={() => finishDelete("purge")}>
                删到回收站
              </button>
              <button type="button" className="btn primary" onClick={() => finishDelete("release")}>
                移到桌面
              </button>
            </div>
          </div>
        </div>
      )}

      <ConfirmDialog
        open={confirmDelete !== null}
        title={`删除「${
          confirmDelete?.title ||
          (confirmDelete ? folderLabel(confirmDelete.path) : "")
        }」？`}
        onCancel={() => setConfirmDelete(null)}
        onConfirm={() => {
          const target = confirmDelete;
          setConfirmDelete(null);
          if (target) {
            void fenceDelete(target.id).catch((problem) => setError(String(problem)));
          }
        }}
      >
        {confirmDelete?.kind === "bucket"
          ? "收纳盒里的文件会留在收纳目录里，之后也能从那里找回。"
          : "只删掉这个格子，文件夹和里面的文件不受影响。"}
      </ConfirmDialog>
    </div>
  );
}

export function GesturePage({ config, update }: { config: Config; update: UpdateConfig }) {
  const g: GestureSettings = config.launcher.gestures ?? {
    enabled: true,
    invertDirection: false,
    triggerDistance: 80,
    showTrail: true,
  };
  const set = (patch: Partial<GestureSettings>) =>
    update({ launcher: { gestures: { ...g, ...patch } } });

  return (
    <div className="stack">
      <Card title="鼠标手势">
        <Row label="启用手势">
          <Toggle checked={g.enabled} onChange={(value) => set({ enabled: value })} />
        </Row>
        <Row label="方向" hint="向左滑动对应哪个动作">
          <Segmented
            value={g.invertDirection ? "leftForward" : "leftBack"}
            options={[
              { value: "leftBack", label: "左滑 → 后退" },
              { value: "leftForward", label: "左滑 → 前进" },
            ]}
            onChange={(value) => set({ invertDirection: value === "leftForward" })}
          />
        </Row>
        <Row label="触发距离" hint="横向滑动超过这个距离才执行">
          <Slider
            value={g.triggerDistance}
            min={20}
            max={200}
            step={5}
            onChange={(value) => set({ triggerDistance: value })}
            format={(value) => `${value} px`}
          />
        </Row>
        <Row label="显示轨迹" hint="按住右键滑动时画出轨迹线，并在光标旁显示方向">
          <Toggle checked={g.showTrail} onChange={(value) => set({ showTrail: value })} />
        </Row>
      </Card>
    </div>
  );
}

/// 「通用」里的存储卡片：缩略图等缓存的占用与清理。
function StorageCard() {
  const [items, setItems] = useState<CacheInfo[]>([]);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);

  const load = () => {
    void cacheStats()
      .then(setItems)
      .catch(() => setItems([]));
  };
  useEffect(load, []);

  const total = items.reduce((sum, item) => sum + item.bytes, 0);

  const clear = async () => {
    setBusy(true);
    setNote(null);
    try {
      const freed = await cacheClear();
      setNote(freed > 0 ? `已释放 ${formatBytes(freed)}` : "缓存已经是空的");
      load();
    } catch (error) {
      setNote(String(error));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Card title="存储">
      {items.length === 0 ? (
        <EmptyState title="还没有缓存" hint="打开过的缩略图会缓存在这里。" />
      ) : (
        items.map((item) => (
          <Row
            key={item.key}
            label={item.label}
            hint={`${item.hint} · ${formatBytes(item.bytes)}，${item.files} 个文件`}
          >
            <button type="button" className="btn" onClick={() => void openPath(item.path)}>
              打开文件夹
            </button>
          </Row>
        ))
      )}

      <Row label="合计">
        <span className="sub">{formatBytes(total)}</span>
      </Row>

      <div style={{ marginTop: 12, display: "flex", gap: 10, alignItems: "center" }}>
        <button type="button" className="btn" disabled={busy || total === 0} onClick={() => void clear()}>
          {busy ? "清理中…" : "清空缓存"}
        </button>
        <span className="row-hint" style={{ margin: 0 }}>
          {note ?? "删掉不会丢东西，只是下次打开要重新生成"}
        </span>
      </div>
    </Card>
  );
}
