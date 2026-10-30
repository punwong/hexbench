import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { Crosshair, Eye, EyeOff, MessageSquare, Rocket } from "lucide-react";

import type { CacheInfo, LauncherCategoryInfo } from "./api";
import type {
  Config,
  GuardSettings,
  HotkeySettings,
  LauncherSettings,
  MonitorDto,
  Phrase,
  PhraseSettings,
  RunningApp,
  Snapshot,
  TriggerSettings,
  UiSettings,
  UpdateSettings,
} from "./types";
import {
  cacheClear,
  cacheStats,
  copyText,
  launcherCategoryList,
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
  togglePhrasesPanel,
} from "./api";
import { TriggerPreview } from "./components/TriggerPreview";
import { Card, Row, Segmented, Slider, Tabs, Toggle } from "./components/ui";

export type Patch = {
  enabled?: boolean;
  trigger?: Partial<TriggerSettings>;
  guard?: Partial<GuardSettings>;
  hotkeys?: Partial<HotkeySettings>;
  phrases?: Partial<PhraseSettings>;
  launcher?: Partial<LauncherSettings>;
  updates?: Partial<UpdateSettings>;
  ui?: Partial<UiSettings>;
};

export type UpdateConfig = (patch: Patch) => void;

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
  onOpen: (page: "edge" | "phrases" | "launcher") => void;
}) {
  const desktops = snap.desktopCount;
  return (
    <div className="stack">
      <Card title="工具">
        <div className="tools">
          <ToolRow
            icon={<Rocket size={18} />}
            name="快捷面板"
            hint="一个键呼出：按分类找文件、搬运文件"
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
            icon={<MessageSquare size={18} />}
            name="常用语"
            hint="吸附在聊天应用旁边的话术面板，点一下就填进输入框"
            enabled={config.phrases.enabled}
            onToggle={(value) => update({ phrases: { enabled: value } })}
            onOpen={() => onOpen("phrases")}
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

export function PhrasesPage({
  config,
  update,
}: {
  config: Config;
  update: UpdateConfig;
}) {
  const phrases = config.phrases;
  const [picking, setPicking] = useState(false);
  const [tab, setTab] = useState<"settings" | "phrases">("settings");

  const setItems = (items: Phrase[]) => update({ phrases: { items } });
  const patch = (id: string, part: Partial<Phrase>) =>
    setItems(phrases.items.map((item) => (item.id === id ? { ...item, ...part } : item)));

  return (
    <div className="stack">
      <Tabs
        value={tab}
        options={[
          { value: "settings", label: "设置" },
          { value: "phrases", label: "话术" },
        ]}
        onChange={setTab}
      />
      {tab === "settings" ? (
      <Card
        title="吸附"
        action={
          <button
            type="button"
            className="btn"
            onClick={() => setPicking(true)}
          >
            选择应用
          </button>
        }
      >
        <Row label="跟随应用" hint="它在前台时面板贴到窗口旁边；不选则面板固定不动">
          <span className="sub">{phrases.target || "未选择"}</span>
        </Row>
        {phrases.targetClass && (
          <Row label="窗口类名" hint="应用改名也不影响，比 exe 名更稳">
            <span className="sub">{phrases.targetClass}</span>
          </Row>
        )}
        <Row label="点击话术" hint="默认只填入输入框；选「填入并发送」会再按一次回车">
          <Segmented
            value={phrases.sendOnClick ? "send" : "fill"}
            options={[
              { value: "fill", label: "只填入" },
              { value: "send", label: "填入并发送" },
            ]}
            onChange={(value) => update({ phrases: { sendOnClick: value === "send" } })}
          />
        </Row>
        <Row label="发送按键" hint="有的聊天软件用 Ctrl+回车 才是发送">
          <Segmented
            value={phrases.sendKey}
            options={[
              { value: "enter", label: "回车" },
              { value: "ctrlEnter", label: "Ctrl+回车" },
            ]}
            onChange={(value) =>
              update({ phrases: { sendKey: value as "enter" | "ctrlEnter" } })
            }
          />
        </Row>
        <Row label="外侧 / 内侧" hint="内侧会叠在目标窗口上，从窗口边缘往里展开">
          <Segmented
            value={phrases.inside ? "inside" : "outside"}
            options={[
              { value: "outside", label: "外侧" },
              { value: "inside", label: "内侧" },
            ]}
            onChange={(value) => update({ phrases: { inside: value === "inside" } })}
          />
        </Row>
        <Row label="贴哪一侧">
          <Segmented
            value={phrases.side}
            options={[
              { value: "right", label: "右侧" },
              { value: "left", label: "左侧" },
            ]}
            onChange={(side) => update({ phrases: { side } })}
          />
        </Row>
        <div style={{ display: "flex", gap: 8, marginTop: 14 }}>
          <button type="button" className="btn primary" onClick={() => void togglePhrasesPanel()}>
            显示面板
          </button>
          {phrases.target && (
            <button
              type="button"
              className="btn"
              onClick={() =>
                update({ phrases: { target: "", targetClass: "", targetTitle: "" } })
              }
            >
              取消吸附
            </button>
          )}
        </div>
      </Card>
      ) : (
      <Card
        title="话术"
        action={
          <button
            type="button"
            className="btn"
            onClick={() =>
              setItems([
                ...phrases.items,
                { id: `p${Date.now()}`, title: "", text: "" },
              ])
            }
          >
            添加一条
          </button>
        }
      >
        {phrases.items.length === 0 && (
          <p className="sub" style={{ margin: 0 }}>
            还没有话术，点右上角添加。
          </p>
        )}
        <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
          {phrases.items.map((item) => (
            <div className="phrase-row" key={item.id}>
              <textarea
                className="phrase-text"
                rows={2}
                value={item.text}
                placeholder="输入话术内容"
                onChange={(event) => patch(item.id, { text: event.target.value })}
              />
              <button
                type="button"
                className="btn"
                onClick={() =>
                  setItems(phrases.items.filter((entry) => entry.id !== item.id))
                }
              >
                删除
              </button>
            </div>
          ))}
        </div>
      </Card>
      )}

      {picking && (
        <AppPicker
          title="吸附到哪个应用"
          hint="只在它处于前台时贴过去；也可以点「拾取窗口」直接点屏幕上的程序。"
          onPick={(app) => {
            update({
              phrases: {
                target: app.name,
                targetClass: app.class ?? "",
                targetTitle: app.title ?? "",
              },
            });
            setPicking(false);
          }}
          onClose={() => setPicking(false)}
        />
      )}
    </div>
  );
}

/** 热键按工具分组：每个工具一组，工具页里不再重复配置。 */
const HOTKEY_GROUPS: Array<{
  tool: string;
  hint: string;
  rows: Array<{ key: "pause" | "phrasesPanel" | "launcher"; label: string; hint: string }>;
}> = [
  {
    tool: "快捷面板",
    hint: "一个键呼出：文件分类 / 快速访问 / 内置网页",
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
  {
    tool: "常用语",
    hint: "话术面板的显示与隐藏",
    rows: [
      { key: "phrasesPanel", label: "显示 / 隐藏面板", hint: "在聊天窗口旁边叫出话术面板" },
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

    document.addEventListener("mousemove", onMove);
    document.addEventListener("mouseup", onUp);
    return () => {
      document.removeEventListener("mousemove", onMove);
      document.removeEventListener("mouseup", onUp);
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
  tab: "items" | "folders" | "sites";
  onTabChange: (tab: "items" | "folders" | "sites") => void;
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

  return (
    <div className="stack">
      <Tabs
        value={tab}
        onChange={(value) => onTabChange(value as "items" | "folders" | "sites")}
        options={[
          { value: "items", label: "快捷启动" },
          { value: "folders", label: "快速访问" },
          { value: "sites", label: "内置网页" },
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
                      hidden={categories[index].hidden}
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
                    update({
                      launcher: {
                        sites: patched.map((item, i) =>
                          i === index && !item.name.trim() ? { ...item, name: title } : item,
                        ),
                      },
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
