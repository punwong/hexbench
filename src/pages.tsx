import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import {
  Crosshair,
  Eye,
  EyeOff,
  FolderOpen,
  Inbox,
  LayoutGrid,
  Plus,
  Rocket,
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
  pickApp,
  pickFolder,
  runningApps,
  syncPull,
  syncPush,
  syncSave,
  syncTest,
} from "./api";
import { TriggerPreview } from "./components/TriggerPreview";
import { Card, Row, Segmented, Slider, Tabs, Toggle } from "./components/ui";

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
  enabled,
  onToggle,
  onOpen,
}: {
  icon: ReactNode;
  name: string;
  hint: string;
  enabled: boolean;
  onToggle: (value: boolean) => void;
  onOpen: () => void;
}) {
  return (
    <div className="tool">
      <span className="tool-icon">{icon}</span>
      <span className="tool-text">
        <span className="tool-name">{name}</span>
        <span className="tool-hint">{hint}</span>
      </span>
      {enabled && (
        <button type="button" className="btn" onClick={onOpen}>
          设置
        </button>
      )}
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
            hint="一个键呼出：按分类找文件、搬运文件，还能存收藏（便签和图片）"
            enabled={config.launcher.enabled}
            onToggle={(value) => update({ launcher: { enabled: value } })}
            onOpen={() => onOpen("launcher")}
          />

          <ToolRow
            icon={<Crosshair size={18} />}
            name="边缘桌面"
            hint={
              desktops
                ? `鼠标推到屏幕最外侧边缘就切虚拟桌面（当前 ${desktops} 个）`
                : "鼠标推到屏幕最外侧边缘就切虚拟桌面"
            }
            enabled={config.enabled}
            onToggle={(value) => update({ enabled: value })}
            onOpen={() => onOpen("edge")}
          />
          <ToolRow
            icon={<LayoutGrid size={18} />}
            name="桌面格子"
            hint="桌面上摆几个绑定文件夹的小组件，文件拖进去就算整理好"
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
  testMode,
  onTest,
}: {
  config: Config;
  update: UpdateConfig;
  monitors: MonitorDto[];
  onPreview: () => void;
  testMode: boolean;
  onTest: (value: boolean) => void;
}) {
  const [tab, setTab] = useState<"trigger" | "guards">("trigger");

  return (
    <div className="stack">
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
          <Card title="测试模式">
            <Row label="只高亮，不切换" hint="先确认位置，再关掉去实际用">
              <Toggle checked={testMode} onChange={onTest} />
            </Row>
          </Card>
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
        title="触发范围"
        action={
          <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
            <span className="sub">
              {positionLabel} {percent}% · {sidesLabel}
              {corners ? "（四角）" : ""}
            </span>
            <button type="button" className="btn" onClick={onPreview}>
              叠到屏幕上
            </button>
          </div>
        }
      >
        <TriggerPreview monitors={monitors} trigger={trigger} />
        <div className="legend">
          <span>
            <i className="swatch area" />
            触发带（屏高的 {percent}%）
          </span>
          <span>
            <i className="swatch edge" />
            可触发边缘
          </span>
        </div>
        <p className="sub" style={{ marginTop: 10, marginBottom: 0 }}>
          鼠标要贴到边缘那一列、并停满下面的「停留阈值」，才会切。点「叠到屏幕上」可以把它投到真实屏幕上看 5 秒。
        </p>
      </Card>

      <Card title="触发带">
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
        <Row label="启用边缘" hint="左右两侧可以单独关">
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
      </Card>

      <Card title="时序">
        <Row label="停留阈值" hint="进入后停留多久才算数">
          <Slider
            value={config.trigger.dwellMs}
            min={0}
            max={500}
            step={10}
            onChange={(value) => update({ trigger: { dwellMs: value } })}
            format={(value) => `${value} ms`}
          />
        </Row>
        <Row label="冷却" hint="两次触发的最小间隔">
          <Slider
            value={config.trigger.cooldownMs}
            min={200}
            max={2000}
            step={50}
            onChange={(value) => update({ trigger: { cooldownMs: value } })}
            format={(value) => `${value} ms`}
          />
        </Row>
        <Row label="轮询间隔" hint="越小越跟手，也越费电">
          <Slider
            value={config.trigger.pollMs}
            min={20}
            max={100}
            step={10}
            onChange={(value) => update({ trigger: { pollMs: value } })}
            format={(value) => `${value} ms`}
          />
        </Row>
      </Card>

      <Card title="光标回位">
        <Row label="触发后向内推" hint="同时充当“离开触发区”，避免连切">
          <Toggle
            checked={config.trigger.restoreCursor}
            onChange={(value) => update({ trigger: { restoreCursor: value } })}
          />
        </Row>
        <Row label="内推距离">
          <Slider
            value={config.trigger.restoreOffsetPx}
            min={8}
            max={200}
            step={4}
            onChange={(value) => update({ trigger: { restoreOffsetPx: value } })}
            format={(value) => `${value} px`}
          />
        </Row>
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

        <Row label="光标在任务栏上" hint="开始按钮、托盘、显示桌面都不抢">
          <Toggle
            checked={config.guard.excludeTaskbar}
            onChange={(value) => update({ guard: { excludeTaskbar: value } })}
          />
        </Row>
        <Row label="锁屏时" hint="安全桌面下直接暂停">
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
            这些程序在前台时不触发。比如远程桌面、窗口化的游戏、模拟器。
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
          hint="点一下加入黑名单；也可以点「拾取窗口」后直接点屏幕上的程序。"
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
        把配置传到你的 WebDAV 目录，换一台电脑拉回来即可。密码用本机 DPAPI 加密保存，不会上传。
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
      <Row label="密码" hint="留空表示沿用已保存的密码">
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
          onClick={() => run("上传", async () => (await remember(), await syncPush()))}
        >
          上传配置
        </button>
        <button
          type="button"
          className="btn"
          disabled={busy}
          onClick={() => run("拉取", async () => (await remember(), await syncPull()))}
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
      <SyncCard config={config} />
      <Card title="外观">
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

      <Card title="启动">
        <Row label="开机自动启动" hint="只进托盘，不弹窗">
          <Toggle checked={snap.autostart} onChange={onAutostart} />
        </Row>

      </Card>

      <Card title="显示器">
        <Row label="重新读取布局" hint="插拔显示器后如果没自动刷新，点一下">
          <button type="button" className="btn" onClick={onRefresh}>
            刷新
          </button>
        </Row>
        <Row label="当前布局">
          <span className="sub">
            {snap.monitors.map((m) => `${m.w}×${m.h}`).join(" + ") || "—"}
          </span>
        </Row>
      </Card>

      <Card title="配置">
        <div className="row-hint" style={{ marginBottom: 8 }}>
          所有设置存在这一个文件里，跨设备同步以后也用它。
        </div>
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

function UpdateCard({
  version,
  endpoint,
  onSaveEndpoint,
  state,
  onCheck,
  onDownload,
  onInstall,
}: {
  version: string;
  endpoint: string;
  onSaveEndpoint: (value: string) => void;
  state: { phase: string; version: string | null; progress: number | null; error: string | null };
  onCheck: () => void;
  onDownload: () => void;
  onInstall: () => void;
}) {
  const [endpointDraft, setEndpointDraft] = useState(endpoint);

  useEffect(() => {
    setEndpointDraft(endpoint);
  }, [endpoint]);

  const busy = state.phase === "checking" || state.phase === "downloading";
  const status =
    state.phase === "checking"
      ? "检查中…"
      : state.phase === "available"
        ? `发现新版本 ${state.version}`
        : state.phase === "downloading"
          ? `下载中 ${state.progress ?? 0}%`
          : state.phase === "ready"
            ? `已下载 ${state.version}，重启后生效`
            : state.phase === "error"
              ? `更新失败：${state.error ?? "未知错误"}`
              : "";

  return (
    <Card title="更新">
      <p className="sub" style={{ marginTop: 0 }}>
        侧边栏会提示新版本：点一下开始下载，下载完成后再点「更新并重启」。
      </p>
      <Row label="当前版本">
        <span className="sub">{version}</span>
      </Row>
      <Row label="更新源" hint="留空用内置地址；也可以填自己的镜像（需 https）">
        <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
          <input
            className="phrase-text"
            style={{ width: 330 }}
            value={endpointDraft}
            placeholder="https://…/latest.json"
            onChange={(event) => setEndpointDraft(event.target.value)}
          />
          <button type="button" className="btn" onClick={() => onSaveEndpoint(endpointDraft.trim())}>
            保存
          </button>
        </div>
      </Row>
      <div style={{ display: "flex", gap: 8, marginTop: 12, alignItems: "center" }}>
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
        <div className="update-bar" style={{ marginTop: 10 }}>
          <i style={{ width: `${state.progress ?? 0}%` }} />
        </div>
      )}
      {status && (
        <p className="sub" style={{ marginBottom: 0 }}>
          {status}
        </p>
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

export function AboutPage({
  snap,
  config,
  update,
  updateState,
  onCheck,
  onDownload,
  onInstall,
}: {
  snap: Snapshot;
  config: Config;
  update: UpdateConfig;
  updateState: { phase: string; version: string | null; progress: number | null; error: string | null };
  onCheck: () => void;
  onDownload: () => void;
  onInstall: () => void;
}) {
  const [copyState, setCopyState] = useState("");

  return (
    <div className="stack">
      <UpdateCard
        version={snap.version}
        endpoint={config.updates.endpoint}
        onSaveEndpoint={(value) => update({ updates: { endpoint: value } })}
        state={updateState}
        onCheck={onCheck}
        onDownload={onDownload}
        onInstall={onInstall}
      />
      <Card title="关于 HexBench">
        <dl className="kv">
          <dt>版本</dt>
          <dd>{snap.version}</dd>
          <dt>许可证</dt>
          <dd>MIT</dd>
          <dt>定位</dt>
          <dd>托盘常驻的 Windows 效率工具箱</dd>
          <dt>首个工具</dt>
          <dd>边缘桌面</dd>
        </dl>
        <div className="footnote">
          项目名与图标不在 MIT 授权范围内；衍生分发请改名换图标。
        </div>
      </Card>

      <Card title="诊断">
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
        <div style={{ marginTop: 14, display: "flex", gap: 10, alignItems: "center" }}>
          <button
            type="button"
            className="btn"
            onClick={() => {
              void (async () => {
                try {
                  await copyText(await diagnostics());
                  setCopyState("已复制，可以直接粘贴给我");
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
      </Card>
    </div>
  );
}

/** 热键按工具分组：每个工具一组，工具页里不再重复配置。 */
const HOTKEY_GROUPS: Array<{
  tool: string;
  hint: string;
  rows: Array<{ key: "pause" | "launcher"; label: string; hint: string }>;
}> = [
  {
    tool: "快捷面板",
    hint: "一个键呼出：文件分类 / 快速访问 / 收藏 / 内置网页",
    rows: [
      { key: "launcher", label: "呼出面板", hint: "任何时候按一下就能呼出" },
    ],
  },
  {
    tool: "边缘桌面",
    hint: "停掉边缘切换不用打开窗口",
    rows: [
      { key: "pause", label: "暂停 / 继续", hint: "随手停掉边缘切换，不用打开窗口" },
    ],
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
      aria-label="设置热键"
    >
      {listening ? "按下组合键…" : "设置"}
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
      <p className="sub" style={{ margin: 0 }}>
        快捷键按工具分组，工具自己的设置页里不再单独配置。点「设置」后按下组合键，Esc 取消；改完立即生效。
      </p>

      {HOTKEY_GROUPS.map((group) => (
        <Card key={group.tool} title={group.tool}>
          <p className="sub" style={{ marginTop: 0 }}>
            {group.hint}
          </p>
          {group.rows.map((row) => (
            <Row key={row.key} label={row.label} hint={row.hint}>
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

      <Card title="说明">
        <p className="sub" style={{ margin: 0, lineHeight: 1.8 }}>
          如果组合键被其他程序占用，注册会失败并在上方提示——换一个组合即可。
          <br />
          工具关掉时，它的快捷键不会生效，但配置会保留。
        </p>
      </Card>
    </div>
  );
}

/**
 * 快捷面板：一个键呼出的面板。设置分成三块，对应三个标签：
 *   快捷启动 —— 根目录下的每个子文件夹就是一个分类；
 *   快速访问 —— 另外钉住的几个普通文件夹；
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
      {hidden ? <EyeOff size={15} /> : <Eye size={15} />}
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
  const folders = config.launcher.folders ?? [];
  const hidden = config.launcher.hiddenCategories ?? [];
  const [categories, setCategories] = useState<LauncherCategoryInfo[]>([]);

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
  // 清空要点两下：第一下只是把按钮变成「确认清空」。
  const [clearArmed, setClearArmed] = useState(false);
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
    if (!clearArmed) {
      setClearArmed(true);
      window.setTimeout(() => setClearArmed(false), 3000);
      return;
    }
    setClearArmed(false);
    setClearing(true);
    try {
      const count = await notesClearAll();
      setNotesNote(count ? `已把 ${count} 条收藏移到回收站` : "没有可清空的收藏");
    } catch (error) {
      setNotesNote(String(error));
    } finally {
      setClearing(false);
    }
  };

  return (
    <div className="stack">
      <Tabs
        value={tab}
        onChange={(value) =>
          onTabChange(value as "items" | "folders" | "sites" | "notes")
        }
        options={[
          { value: "items", label: "快捷启动" },
          { value: "folders", label: "快速访问" },
          { value: "sites", label: "内置网页" },
          { value: "notes", label: "收藏" },
        ]}
      />

      {tab === "items" ? (
        <Card title="快捷启动">
          <p className="sub" style={{ marginTop: 0 }}>
            选一个根目录，它下面的每个子文件夹就是一个分类（比如 常用 / 软件 / 设计）。
            面板左侧按下面的顺序生成导航，拖动可以调整。
          </p>
          <Row label="根目录" hint="例如 D:\启动器，里面放几个子文件夹当分类">
            <div className="hotkey-cell">
              <code>{root || "未选择"}</code>
              <button type="button" className="btn" onClick={() => void chooseRoot()}>
                选择
              </button>
              <button
                type="button"
                className="btn"
                onClick={() => update({ launcher: { root: "", categoryOrder: [] } })}
              >
                清空
              </button>
            </div>
          </Row>

          {root && (
            <div style={{ marginTop: 12 }}>
              <div className="row-hint" style={{ marginBottom: 8 }}>
                面板里的顺序（拖动调整）；眼睛控制它在面板上显不显示
              </div>

              {/* 「全部」：固定在第一条，只能显示 / 隐藏 */}
              <div className="sortable-row" style={{ cursor: "default" }}>
                <span className="grip" style={{ opacity: 0.25, cursor: "default" }}>
                  ⠿
                </span>
                <span className="sortable-label">全部</span>
                <span className="sub">所有分类合在一起</span>
                <span className="sortable-actions">
                  <EyeButton
                    hidden={hidden.includes("__all__")}
                    onClick={() => toggleHidden("__all__")}
                  />
                </span>
              </div>

              <div style={{ height: 6 }} />

              {categories.length === 0 ? (
                <div className="row-hint">这个目录下还没有子文件夹。</div>
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

          <Row
            label="呼出位置"
            hint="打开后贴着鼠标出现；关掉则固定在该屏幕的中间偏上"
          >
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

          <Row label="记住拖出来的面板大小">
            <Toggle
              checked={config.launcher.panelRememberSize ?? true}
              onChange={(next) => update({ launcher: { panelRememberSize: next } })}
            />
          </Row>
          <Row label="面板宽度" hint="占所在屏幕工作区宽度的百分比">
            <Slider
              value={config.launcher.widthPercent}
              min={25}
              max={80}
              step={1}
              onChange={(value) => update({ launcher: { widthPercent: value } })}
              format={(value) => `${value}%`}
            />
          </Row>
          <Row label="面板高度" hint="占所在屏幕工作区高度的百分比">
            <Slider
              value={config.launcher.heightPercent}
              min={30}
              max={80}
              step={1}
              onChange={(value) => update({ launcher: { heightPercent: value } })}
              format={(value) => `${value}%`}
            />
          </Row>
          <Row
            label="记住上次位置"
            hint="呼出时回到上次浏览的文件夹，不用每次重新选"
          >
            <Toggle
              checked={config.launcher.rememberLocation}
              onChange={(value) => update({ launcher: { rememberLocation: value } })}
            />
          </Row>
        </Card>
      ) : tab === "sites" ? (
        <Card title="内置网页">
          <p className="sub" style={{ marginTop: 0 }}>
            这里加的站点会出现在网页窗口的顶栏，点一下直接打开（真 WebView，不是 iframe，
            所以 Google、ChatGPT 这类也能开）。面板收起时网页会一起关掉，不占内存。
          </p>
          <Row label="启用网页窗口" hint="关掉之后面板只显示文件夹，不会创建任何 WebView">
            <Toggle
              checked={config.launcher.webEnabled !== false}
              onChange={(value) => update({ launcher: { webEnabled: value } })}
            />
          </Row>

          <Row label="收起后保留" hint="面板收起后网页继续运行多少秒（0 = 立刻关闭）。问 AI 时建议留几十秒">
            <Slider
              value={config.launcher.siteKeepAliveSecs ?? 90}
              min={0}
              max={300}
              step={10}
              onChange={(value) => update({ launcher: { siteKeepAliveSecs: value } })}
              format={(value) => (value === 0 ? "立刻关闭" : `${value} 秒`)}
            />
          </Row>

          <Row label="网页窗口宽度" hint="占所在屏幕工作区宽度的百分比">
            <Slider
              value={config.launcher.siteWidthPercent ?? 24}
              min={10}
              max={90}
              step={1}
              onChange={(value) => update({ launcher: { siteWidthPercent: value } })}
              format={(value) => `${value}%`}
            />
          </Row>
          <Row label="网页窗口高度" hint="占所在屏幕工作区高度的百分比，可以比呼出面板高很多">
            <Slider
              value={config.launcher.siteHeightPercent ?? 85}
              min={20}
              max={100}
              step={1}
              onChange={(value) => update({ launcher: { siteHeightPercent: value } })}
              format={(value) => `${value}%`}
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

          {(config.launcher.sites ?? []).length === 0 && (
            <div className="row-hint" style={{ marginBottom: 10 }}>
              还没有添加网址。
            </div>
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
        </Card>
      ) : tab === "notes" ? (
        <Card title="收藏">
          <p className="sub" style={{ marginTop: 0 }}>
            收藏是面板里的便签：写 Markdown 文字，也可以把图片拖进去。每个笔记都是一个
            .md 文件，图片放在旁边的 images 文件夹里，整个目录可以直接用别的编辑器打开、
            或者丢进同步盘。点「发送」会收起面板，把文字逐字输入到呼出前的窗口。
          </p>
          <Row label="启用收藏" hint="在呼出面板左侧显示「收藏」这一栏">
            <Toggle
              checked={config.notes.enabled !== false}
              onChange={(value) => update({ notes: { enabled: value } })}
            />
          </Row>
          <Row
            label="保存位置"
            hint="默认是「文档\HexBench 收藏」，也可以换成同步盘里的文件夹"
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
          <Row label="清空全部收藏" hint="笔记和图片都会进回收站，之后还能从回收站找回">
            <button
              type="button"
              className="btn"
              disabled={clearing}
              onClick={() => void clearNotes()}
            >
              {clearing ? "清空中…" : clearArmed ? "确认清空" : "清空…"}
            </button>
          </Row>
          {notesNote && <div className="row-hint">{notesNote}</div>}
        </Card>
      ) : (
        <Card title="快速访问">
          <p className="sub" style={{ marginTop: 0 }}>
            这里的文件夹会出现在面板左侧，点一下直接跳过去；拖动可以调整顺序。
            往里拖文件就是移动进那个目录，往外拖就是复制或移动出去。
          </p>

          {folders.length === 0 ? (
            <div className="row-hint" style={{ marginBottom: 10 }}>
              还没有添加任何文件夹。
            </div>
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
                onClick={() => update({ launcher: { folders: [] } })}
              >
                全部清空
              </button>
            )}
          </div>
        </Card>
      )}
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
        按规则把来源目录里的文件收进这个盒子。盒子自己和收纳目录永远不会被再收一次；
        一次整理算一步撤销（Ctrl+Z 能退）。
      </p>
      <Row label="自动整理" hint="来源目录一有变化就按规则收；关掉只能手动点「立即整理」">
        <Toggle
          checked={fence.rulesAuto}
          onChange={(value) => void fenceUpdate(fence.id, { rulesAuto: value })}
        />
      </Row>
      {draft.length === 0 && (
        <div className="row-hint">
          还没有规则。没有规则时它就是普通收纳盒：拖什么进来收什么。
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
              大于 MB
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
              小于 MB
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
              早于 N 天
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
export function DesktopPage({ config, update }: { config: Config; update: UpdateConfig }) {
  const desktop = config.desktop;
  const fences = desktop.fences ?? [];
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // 哪一格的整理规则展开着（同一时间只开一个）。
  const [openRules, setOpenRules] = useState<string | null>(null);
  // 删除收纳盒前问文件怎么处理；普通格子直接确认删掉。
  const [deleting, setDeleting] = useState<{
    fence: FenceSettings;
    info: FenceBoxInfo;
  } | null>(null);

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
    if (!window.confirm(`删除「${fence.title || folderLabel(fence.path)}」这个格子？`)) return;
    void fenceDelete(fence.id).catch((problem) => setError(String(problem)));
  };

  const finishDelete = (mode: "release" | "purge" | "keep") => {
    const target = deleting;
    setDeleting(null);
    if (!target) return;
    void fenceDelete(target.fence.id, mode).catch((problem) => setError(String(problem)));
  };

  return (
    <div className="stack">
      <Card title="桌面格子">
        <p className="sub" style={{ marginTop: 0 }}>
          在桌面上摆几个绑定文件夹的小组件：文件拖进去就算整理好，系统桌面图标照常使用。
        </p>
        <Row label="开启桌面格子" hint="关掉会销毁所有格子窗口，位置和设置保留">
          <Toggle
            checked={desktop.enabled}
            onChange={(value) => update({ desktop: { enabled: value } })}
          />
        </Row>
        <Row label="按 Win+D 时保持可见" hint="关掉就跟着系统桌面一起藏起来">
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
        <Row
          label="原生渲染（Beta）"
          hint="内存约为 WebView 版的十分之一；拖放、右键菜单、边角缩放、跟随桌面隐藏都已支持，还差 Alt+滚轮缩放；关掉即切回"
        >
          <Toggle
            checked={desktop.native}
            onChange={(value) => update({ desktop: { native: value } })}
          />
        </Row>
      </Card>

      <Card title="外观">
        <Row label="不透明度" hint="越低背后的壁纸 / 磨砂透得越多；太低字会和背景糊在一起">
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
        <Row label="磨砂（模糊背景）" hint="关掉就是纯透明玻璃：壁纸原样透出来，不模糊">
          <Toggle
            checked={frosted}
            onChange={(value) => {
              setFrosted(value);
              applyAppearance(opacity, value);
            }}
          />
        </Row>
        <p className="sub" style={{ marginBottom: 0 }}>
          所有格子共用这一份外观，改完立刻生效。系统不支持磨砂时，卡片会自动变成近乎纯色。
        </p>
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
          <div className="empty-inline">
            <LayoutGrid size={20} />
            <span>
              还没有格子。绑一个文件夹，或者新建一个收纳盒：拖进去的文件真的会被收起来。
            </span>
          </div>
        ) : (
          <div className="fence-list">
            {fences.map((fence) => {
              const isBox = fence.kind === "bucket";
              const ruleCount = fence.rules?.length ?? 0;
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
                          <span className="fence-badge">{ruleCount > 0 ? "整理盒" : "收纳盒"}</span>
                        )}
                        {fence.rolled ? "（卷起）" : ""}
                      </span>
                      <span className="fence-path mono" title={fence.path}>
                        {isBox ? `收纳目录 · ${fence.path}` : fence.path || "没有绑定文件夹"}
                      </span>
                      <span className="row-hint">
                        {fence.monitor || "主显示器"} ·{" "}
                        {`${Math.round(fence.w * 100)}% × ${Math.round(fence.h * 100)}%`}
                        {fence.zoom !== 100 ? ` · ${fence.zoom}% 排版` : ""}
                        {isBox && ruleCount > 0
                          ? ` · ${ruleCount} 条规则${fence.rulesAuto ? "（自动）" : "（手动）"}`
                          : ""}
                      </span>
                    </span>
                    <span className="fence-actions">
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
                          <SlidersHorizontal size={15} />
                        </button>
                      )}
                      <button
                        type="button"
                        className="icon-btn"
                        title={isBox ? "打开收纳目录" : "在资源管理器中打开"}
                        disabled={!fence.path}
                        onClick={() => void openPath(fence.path)}
                      >
                        <FolderOpen size={15} />
                      </button>
                      <button
                        type="button"
                        className="icon-btn danger"
                        title="删除这个格子"
                        onClick={() => void askDelete(fence)}
                      >
                        <Trash2 size={15} />
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
          桌面上：拖标题条移动，拖边缘 / 四角缩放，双击标题条卷起；右键文件用系统菜单；
          双击文件夹进入，Alt+滚轮缩放排版。收纳盒里的文件拖出去也能放回桌面。
        </div>
        {error && <div className="row-error">{error}</div>}
        {fences.length >= 8 && (
          <div className="row-hint" style={{ marginTop: 8 }}>
            每多一个格子就多一个 WebView，最多 8 个。
          </div>
        )}
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
              <button type="button" className="btn" onClick={() => finishDelete("purge")}>
                删到回收站
              </button>
              <button type="button" className="btn primary" onClick={() => finishDelete("release")}>
                移到桌面
              </button>
            </div>
            <p className="sub" style={{ marginBottom: 0 }}>
              「保留文件」只删格子，文件继续留在收纳目录里；「移到桌面」会把它们都放到系统桌面。
            </p>
          </div>
        </div>
      )}
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
        <p className="sub" style={{ marginTop: 0 }}>
          在快捷面板的文件区、桌面格子的文件区按住右键横着划：默认向左后退、向右前进，
          松手后执行，不弹右键菜单。格子里的后退 / 前进走的是你进出过的目录。
          只在文件区生效，用右键慢慢点开菜单不受影响。
        </p>
        <Row label="启用手势">
          <Toggle checked={g.enabled} onChange={(value) => set({ enabled: value })} />
        </Row>
        <Row label="方向" hint="向左滑动对应哪个动作">
          <Segmented
            value={g.invertDirection ? "leftForward" : "leftBack"}
            options={[
              { value: "leftBack", label: "左 = 后退" },
              { value: "leftForward", label: "左 = 前进" },
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

/// 设置页里的「缓存」：目前只有缩略图，以后别的工具也能挂在这里。
export function CachePage() {
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
    <div className="stack">
      <Card title="缓存">
        <div className="row-hint" style={{ marginBottom: 12 }}>
          删掉不会丢东西，只是下次打开要重新生成一遍。
        </div>

        {items.length === 0 && <div className="row-hint">还没有任何缓存。</div>}

        {items.map((item) => (
          <div
            key={item.key}
            style={{
              padding: "10px 12px",
              border: "1px solid var(--line)",
              borderRadius: 10,
              marginBottom: 10,
            }}
          >
            <div style={{ display: "flex", alignItems: "baseline", gap: 10 }}>
              <strong style={{ fontSize: 13 }}>{item.label}</strong>
              <span className="sub">
                {formatBytes(item.bytes)} · {item.files} 个文件
              </span>
            </div>
            <div className="row-hint" style={{ margin: "4px 0 6px" }}>
              {item.hint}
            </div>
            <div className="mono" style={{ wordBreak: "break-all", color: "var(--muted)", fontSize: 11.5 }}>
              {item.path}
            </div>
            <div style={{ marginTop: 8 }}>
              <button type="button" className="btn" onClick={() => void openPath(item.path)}>
                打开文件夹
              </button>
            </div>
          </div>
        ))}

        <Row label="合计" hint="所有缓存加起来的占用">
          <span className="sub">{formatBytes(total)}</span>
        </Row>

        <div style={{ marginTop: 12 }}>
          <button type="button" className="btn" disabled={busy || total === 0} onClick={() => void clear()}>
            {busy ? "清理中…" : "清空所有缓存"}
          </button>
        </div>
        {note && (
          <div className="row-hint" style={{ marginTop: 8 }}>
            {note}
          </div>
        )}
      </Card>
    </div>
  );
}
