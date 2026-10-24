import { useEffect, useState } from "react";

import type {
  Config,
  GuardSettings,
  HotkeySettings,
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
  onAppPicked,
  pickApp,
  runningApps,
  syncPull,
  syncPush,
  syncSave,
  syncTest,
  togglePhrasesPanel,
} from "./api";
import { MonitorMap } from "./components/MonitorMap";
import { TriggerPreview } from "./components/TriggerPreview";
import { Card, Row, Segmented, Slider, Toggle } from "./components/ui";

export type Patch = {
  enabled?: boolean;
  trigger?: Partial<TriggerSettings>;
  guard?: Partial<GuardSettings>;
  hotkeys?: Partial<HotkeySettings>;
  phrases?: Partial<PhraseSettings>;
  updates?: Partial<UpdateSettings>;
  ui?: Partial<UiSettings>;
};

export type UpdateConfig = (patch: Patch) => void;

export function Overview({
  snap,
  config,
  update,
  onPause,
  onTest,
  onCreateDesktop,
}: {
  snap: Snapshot;
  config: Config;
  update: UpdateConfig;
  onPause: () => void;
  onTest: (value: boolean) => void;
  onCreateDesktop: () => void;
}) {
  const status = snap.paused ? "已暂停" : snap.testMode ? "测试模式" : "运行中";
  const statusClass = snap.paused ? "warn" : snap.testMode ? "warn" : "ok";

  return (
    <div className="stack">
      {snap.desktopCount === 1 && (
        <div
          className="callout"
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
            gap: 16,
          }}
        >
          <div>
            <b>当前只有 1 个虚拟桌面。</b>
            推到边缘不会有反应——先新建一个。新建后会回到这里，把鼠标推到边缘就能过去。
          </div>
          <button type="button" className="btn primary" onClick={onCreateDesktop}>
            新建虚拟桌面
          </button>
        </div>
      )}

      <div className="grid2">
        <Card
          title="显示器布局"
          action={
            <span className="sub">
              {snap.monitors.length} 块屏 · {snap.regions.length} 段可触发
            </span>
          }
        >
          <MonitorMap
            monitors={snap.monitors}
            regions={snap.regions}
            hit={snap.hit}
            sides={config.trigger.sides}
            dimmed={snap.paused}
          />
          <div className="legend">
            <span>
              <i className="swatch" />
              可触发
            </span>
            <span>
              <i className="swatch solid" />
              命中
            </span>
            <span>
              <i className="swatch grey" />
              接缝 / 已禁用
            </span>
          </div>
        </Card>

        <div className="stack">
          <Card title="运行状态">
            <div className="status">
              <i className={`dot ${statusClass}`} />
              {status}
            </div>
            <div style={{ marginTop: 16 }}>
              <div className="metric-label">本次运行切换</div>
              <div className="big-number">
                {snap.stats.fires}
                <span style={{ fontSize: 13, color: "var(--muted)", marginLeft: 6 }}>
                  次
                </span>
              </div>
              <div
                style={{
                  marginTop: 8,
                  display: "flex",
                  alignItems: "center",
                  justifyContent: "space-between",
                  gap: 10,
                }}
              >
                <span style={{ fontSize: 12.5, color: "var(--muted)" }}>
                  虚拟桌面 {snap.desktopCount ?? "—"} 个
                </span>
                <button
                  type="button"
                  className="btn"
                  style={{ padding: "3px 10px", fontSize: 12 }}
                  onClick={onCreateDesktop}
                >
                  新建
                </button>
              </div>
            </div>
            <div style={{ display: "flex", gap: 8, marginTop: 18 }}>
              <button type="button" className="btn" onClick={onPause}>
                {snap.paused ? "继续" : "暂停"}
              </button>
              <button
                type="button"
                className="btn"
                onClick={() => update({ enabled: !config.enabled })}
              >
                {config.enabled ? "关闭总开关" : "打开总开关"}
              </button>
            </div>
          </Card>

          <Card title="测试模式">
            <Row label="只高亮，不切换" hint="先确认位置，再关掉去实际用">
              <Toggle checked={snap.testMode} onChange={onTest} />
            </Row>
          </Card>
        </div>
      </div>

      <Card title="怎么用">
        <div className="callout">
          <b>1.</b> 把鼠标推到屏幕最外侧边缘的角落区，默认是底部 10%。
          <br />
          <b>2.</b> 停住约 0.1 秒，不要一路划过去。
          <br />
          <b>3.</b> 切过去后光标会被推回 24 px，避免停在那儿连续触发。
        </div>
      </Card>
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
            <i className="swatch" />
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
        <Row label="前台全屏时" hint="游戏、视频、PPT 放映">
          <Toggle
            checked={config.guard.skipFullscreen}
            onChange={(value) => update({ guard: { skipFullscreen: value } })}
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
  fullscreen: "前景是全屏窗口",
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
          <dd>边缘切桌面</dd>
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

  const setItems = (items: Phrase[]) => update({ phrases: { items } });
  const patch = (id: string, part: Partial<Phrase>) =>
    setItems(phrases.items.map((item) => (item.id === id ? { ...item, ...part } : item)));

  return (
    <div className="stack">
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

      <Card
        title="话术"
        action={
          <button
            type="button"
            className="btn"
            onClick={() =>
              setItems([
                ...phrases.items,
                { id: `p${Date.now()}`, title: "新话术", text: "" },
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
              <input
                className="phrase-title"
                value={item.title}
                placeholder="标题"
                onChange={(event) => patch(item.id, { title: event.target.value })}
              />
              <textarea
                className="phrase-text"
                rows={2}
                value={item.text}
                placeholder="要发送的内容"
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

const HOTKEY_ROWS: { key: "pause" | "phrasesPanel"; label: string; hint: string }[] = [
  { key: "pause", label: "暂停 / 继续", hint: "随手停掉边缘切换，不用打开窗口" },
  { key: "phrasesPanel", label: "快捷语面板", hint: "显示 / 隐藏话术面板" },
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
      <Card title="全局热键">
        <p className="sub" style={{ marginTop: 0 }}>
          所有快捷键都在这里注册，工具再多也只抢这一处。点「设置」后按下组合键，Esc 取消。
        </p>
        {HOTKEY_ROWS.map((row) => (
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
        {snap.hotkeyError && <p className="sub" style={{ color: "#b42318" }}>{snap.hotkeyError}</p>}
      </Card>

      <Card title="说明">
        <p className="sub" style={{ margin: 0, lineHeight: 1.8 }}>
          热键修改后立即生效，不需要重启。
          <br />
          如果组合键被其他程序占用，注册会失败并在上方提示——换一个组合即可。
        </p>
      </Card>
    </div>
  );
}