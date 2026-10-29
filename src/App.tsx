import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import type { ReactNode } from "react";
import {
  Crosshair,
  Database,
  Gauge,
  Info,
  Keyboard,
  MessageSquare,
  Rocket,
  Settings,
  Sparkles,
  AlertTriangle,
} from "lucide-react";

import {
  checkUpdate,
  downloadUpdate,
  getSnapshot,
  installDownloaded,
  onUpdateStatus,
  updateStatus,
  openConfigFolder,
  refreshLayout,
  saveConfig,
  setAutostart,
  setTestMode,
  showTriggerOverlay,
} from "./api";
import type { Config, Snapshot } from "./types";
import {
  AboutPage,
  EdgePage,
  LauncherPage,
  CachePage,
  GeneralPage,
  HotkeysPage,
  Overview,
  PhrasesPage,

} from "./pages";
import type { Patch } from "./pages";
import { TitleBar } from "./components/TitleBar";

type Page = "overview" | "edge" | "phrases" | "launcher" | "hotkeys" | "general" | "cache" | "about";

/// Where the updater currently is: nothing, a version to grab, downloading,
/// a package waiting for a restart, or a failure worth showing.
type UpdatePhase = "idle" | "checking" | "available" | "downloading" | "ready" | "error";

const TITLES: Record<Page, { title: string; subtitle: string }> = {
  overview: { title: "概览", subtitle: "按需打开，各占一个设置页" },
  edge: { title: "边缘桌面", subtitle: "触发位置、时序与防误触" },
  phrases: { title: "常用语", subtitle: "吸附在聊天窗口旁，点一下就发出去" },
  launcher: { title: "快捷启动", subtitle: "一个键呼出：启动程序、搬运文件" },
  hotkeys: { title: "热键", subtitle: "所有全局快捷键，集中在一处" },
  general: { title: "通用", subtitle: "外观、启动与配置文件" },
  cache: { title: "缓存", subtitle: "缩略图等缓存的占用与清理" },
  about: { title: "关于", subtitle: "版本、许可证与诊断" },
};

export default function App() {
  const [page, setPage] = useState<Page>("overview");
  const [launcherTab, setLauncherTab] = useState<"items" | "folders" | "sites">("items");
  // 面板里点「去设置」：切到指定页面（可带 tab）。
  useEffect(() => {
    let dispose: (() => void) | undefined;
    void listen<{ page: string; tab: string | null } | null>("navigate", (event) => {
      const target = event.payload?.page ?? "overview";
      const tab = event.payload?.tab ?? null;
      if (target === "launcher" && (tab === "items" || tab === "folders" || tab === "sites")) {
        setLauncherTab(tab as "items" | "folders" | "sites");
      }
      setPage(target as Page);
    }).then((fn) => {
      dispose = fn;
    });
    return () => dispose?.();
  }, []);
  const [snap, setSnap] = useState<Snapshot | null>(null);
  const [config, setConfig] = useState<Config | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [step, setStep] = useState(0);

  const [updatePhase, setUpdatePhase] = useState<UpdatePhase>("idle");
  const [updateVersion, setUpdateVersion] = useState<string | null>(null);
  const [updateProgress, setUpdateProgress] = useState<number | null>(null);
  const [updateError, setUpdateError] = useState<string | null>(null);

  const runCheck = useCallback(async (silent: boolean) => {
    setUpdatePhase("checking");
    setUpdateError(null);
    try {
      const info = await checkUpdate();
      if (!info) {
        setUpdateVersion(null);
        setUpdatePhase("idle");
        return;
      }
      setUpdateVersion(info.version);
      setUpdatePhase("available");
    } catch (reason) {
      setUpdateError(String(reason));
      // A failed automatic check should stay quiet; a manual one should not.
      setUpdatePhase(silent ? "idle" : "error");
    }
  }, []);

  const runDownload = useCallback(async () => {
    setUpdatePhase("downloading");
    setUpdateProgress(0);
    try {
      await downloadUpdate(setUpdateProgress);
      setUpdatePhase("ready");
    } catch (reason) {
      setUpdateError(String(reason));
      setUpdatePhase("error");
    }
  }, []);

  const runInstall = useCallback(async () => {
    try {
      // Hands the package to NSIS, which replaces the files and restarts us.
      await installDownloaded();
    } catch (reason) {
      setUpdateError(String(reason));
      setUpdatePhase("error");
    }
  }, []);

  // Silent update: as soon as a newer version is known, fetch it in the
  // background. The sidebar then only ever offers a single「安装」action.
  useEffect(() => {
    if (updatePhase !== "available" || !updateVersion) return;
    if (updateVersion === config?.updates.dismissedVersion) return;
    void runDownload();
  }, [updatePhase, updateVersion, config?.updates.dismissedVersion, runDownload]);

  // The process checks once at startup; windows read that cached result and
  // follow the live event, so opening the window costs no network round trip.
  useEffect(() => {
    let alive = true;
    let unlisten: (() => void) | undefined;

    const applyStatus = (status: { version: string | null }) => {
      if (!alive) return;
      if (status.version) {
        setUpdateVersion(status.version);
        setUpdatePhase((phase) =>
          phase === "downloading" || phase === "ready" ? phase : "available",
        );
      } else {
        setUpdatePhase((phase) =>
          phase === "downloading" || phase === "ready" ? phase : "idle",
        );
      }
    };

    void (async () => {
      try {
        applyStatus(await updateStatus());
      } catch {
        // A failed read only means no hint; the user can still check manually.
      }
      if (!alive) return;
      unlisten = await onUpdateStatus(applyStatus);
    })();

    return () => {
      alive = false;
      unlisten?.();
    };
  }, []);

  const configRef = useRef<Config | null>(null);
  const saveTimer = useRef<number | undefined>(undefined);

  useEffect(() => {
    let alive = true;
    let timer: number | undefined;

    const poll = async () => {
      try {
        const next = await getSnapshot();
        if (!alive) return;
        setSnap(next);
        if (!configRef.current) {
          configRef.current = next.config;
          setConfig(next.config);
        }
      } catch (reason) {
        if (alive) setError(String(reason));
      }
      // The layout diagram polls the cursor on its own; this only refreshes\n        // config/stats, so it can be slow.\n        if (alive) timer = window.setTimeout(poll, 500);
    };

    void poll();
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, []);



  const update = useCallback((patch: Patch) => {
    const current = configRef.current;
    if (!current) return;
    const next: Config = {
      ...current,
      ...patch,
      trigger: { ...current.trigger, ...patch.trigger },
      guard: { ...current.guard, ...patch.guard },
      hotkeys: { ...current.hotkeys, ...patch.hotkeys },
      phrases: { ...current.phrases, ...patch.phrases },
      launcher: { ...current.launcher, ...patch.launcher },
      updates: { ...current.updates, ...patch.updates },
      ui: { ...current.ui, ...patch.ui },
    };
    configRef.current = next;
    setConfig(next);
    window.clearTimeout(saveTimer.current);
    saveTimer.current = window.setTimeout(() => {
      void saveConfig(next)
        .then(() => setError(null))
        .catch((reason) => setError(String(reason)));
    }, 300);
  }, []);

  useEffect(() => {
    const apply = () => {
      const preference = config?.ui.theme ?? "system";
      const dark =
        preference === "dark" ||
        (preference === "system" &&
          window.matchMedia("(prefers-color-scheme: dark)").matches);
      document.documentElement.dataset.theme = dark ? "dark" : "light";
    };
    apply();
    const query = window.matchMedia("(prefers-color-scheme: dark)");
    query.addEventListener("change", apply);
    return () => query.removeEventListener("change", apply);
  }, [config?.ui.theme]);

  // Snapshot refreshes twice a second; the switch must react on click.
  const [testModeLocal, setTestModeLocal] = useState<boolean | null>(null);
  useEffect(() => {
    if (snap) setTestModeLocal(snap.testMode);
  }, [snap?.testMode]);
  const testMode = testModeLocal ?? snap?.testMode ?? false;

  // A tool that gets switched off takes its settings page with it.
  useEffect(() => {
    if (!config) return;
    if (page === "edge" && !config.enabled) setPage("overview");
    if (page === "phrases" && !config.phrases.enabled) setPage("overview");
    if (page === "launcher" && !config.launcher.enabled) setPage("overview");
  }, [config?.enabled, config?.phrases.enabled, config?.launcher.enabled, page]);

  const finishOnboarding = () => {
    update({ ui: { onboarded: true } });
    void setTestMode(true);
  };

  const status = snap
    ? snap.paused
      ? "已暂停"
      : snap.testMode
        ? "测试模式"
        : config?.enabled
          ? "运行中"
          : "已关闭"
    : "加载中";
  const statusClass = snap?.paused || snap?.testMode ? "warn" : config?.enabled ? "ok" : "";

  const showOnboarding = snap !== null && config !== null && !config.ui.onboarded;

  // A dismissed version stays quiet until a different one shows up.
  const updateDismissed =
    updateVersion !== null && updateVersion === config?.updates.dismissedVersion;
  // Downloading is invisible on purpose: the pill appears only when the
  // package is on disk and one click finishes the job.
  const showUpdatePill =
    updatePhase === "error" || (updatePhase === "ready" && !updateDismissed);

  return (
    <div className="app">
      <TitleBar />
      <div className="shell">
      <aside className="sidebar">
        <div className="nav-group">
          <NavItem page="overview" current={page} onClick={setPage} icon={<Gauge size={16} />} label="概览" />
        </div>

        {config && (config.enabled || config.phrases.enabled || config.launcher.enabled) && (
          <div className="nav-group">
            <div className="nav-label">工具</div>
            {config.launcher.enabled && (
              <NavItem page="launcher" current={page} onClick={setPage} icon={<Rocket size={16} />} label="快捷启动" />
            )}
            {config.enabled && (
              <NavItem page="edge" current={page} onClick={setPage} icon={<Crosshair size={16} />} label="边缘桌面" />
            )}
            {config.phrases.enabled && (
              <NavItem page="phrases" current={page} onClick={setPage} icon={<MessageSquare size={16} />} label="常用语" />
            )}
          </div>
        )}

        <div className="nav-group">
          <div className="nav-label">全局</div>
          <NavItem page="hotkeys" current={page} onClick={setPage} icon={<Keyboard size={16} />} label="热键" />
          <NavItem page="general" current={page} onClick={setPage} icon={<Settings size={16} />} label="通用" />
          <NavItem page="cache" current={page} onClick={setPage} icon={<Database size={16} />} label="缓存" />
          <NavItem page="about" current={page} onClick={setPage} icon={<Info size={16} />} label="关于" />
        </div>

        <div className="sidebar-bottom">
        {showUpdatePill && (
          <div className={`update-pill ${updatePhase}`}>
            {updatePhase === "ready" ? (
              <>
                <div className="update-note">
                  <span className="update-version">新版本 {updateVersion}</span>
                  <span className="update-sub">已下载，装好会重启</span>
                </div>
                <button
                  type="button"
                  className="btn primary update-install"
                  onClick={() => void runInstall()}
                >
                  更新并安装
                </button>
                <button
                  type="button"
                  className="update-dismiss"
                  title="忽略这个版本"
                  onClick={() => {
                    update({ updates: { dismissedVersion: updateVersion ?? "" } });
                  }}
                >
                  ×
                </button>
              </>
            ) : (
              <button
                type="button"
                className="update-action"
                onClick={() => setPage("about")}
              >
                <AlertTriangle size={14} />
                <span>更新失败，点此查看</span>
              </button>
            )}
          </div>
        )}

        <div className="sidebar-foot">v{snap?.version ?? "0.1.0"} · MIT</div>
        </div>
      </aside>

      <main className="main">
        <header className="page-head">
          <div>
            <h1>{TITLES[page].title}</h1>
            <div className="sub">{TITLES[page].subtitle}</div>
          </div>
          <div className="status">
            <i className={`dot ${statusClass}`} />
            {status}
          </div>
        </header>

        <div className="page" key={page}>
        {error && <div className="error">保存失败：{error}</div>}

        {!snap || !config ? (
          <div className="card">正在读取状态…</div>
        ) : page === "overview" ? (
          <Overview snap={snap} config={config} update={update} onOpen={setPage} />
        ) : page === "edge" ? (
          <EdgePage
            config={config}
            update={update}
            monitors={snap.monitors}
            testMode={testMode}
            onTest={(value) => {
              setTestModeLocal(value);
              void setTestMode(value);
            }}
            onPreview={() => {
              void showTriggerOverlay().catch((reason) => setError(String(reason)));
            }}
          />
        ) : page === "launcher" ? (
          <LauncherPage
            config={config}
            update={update}
            tab={launcherTab}
            onTabChange={setLauncherTab}
          />

        ) : page === "phrases" ? (
          <PhrasesPage config={config} update={update} />
        ) : page === "hotkeys" ? (
          <HotkeysPage config={config} update={update} snap={snap} />
        ) : page === "general" ? (
          <GeneralPage
            config={config}
            update={update}
            snap={snap}
            onAutostart={(value) => {
              void setAutostart(value).catch((reason) => setError(String(reason)));
            }}
            onRefresh={() => void refreshLayout()}
            onOpenFolder={() => void openConfigFolder()}
          />
        ) : page === "cache" ? (
          <CachePage />
        ) : (
          <AboutPage
            snap={snap}
            config={config}
            update={update}
            updateState={{
              phase: updatePhase,
              version: updateVersion,
              progress: updateProgress,
              error: updateError,
            }}
            onCheck={() => void runCheck(false)}
            onDownload={() => void runDownload()}
            onInstall={() => void runInstall()}
          />
        )}
        </div>
      </main>
      </div>

      {showOnboarding && (
        <Onboarding
          step={step}
          onNext={() => setStep((value) => value + 1)}
          onFinish={finishOnboarding}
          onSkip={finishOnboarding}
        />
      )}
    </div>
  );
}

function NavItem({
  page,
  current,
  onClick,
  icon,
  label,
}: {
  page: Page;
  current: Page;
  onClick: (page: Page) => void;
  icon: ReactNode;
  label: string;
}) {
  return (
    <button
      type="button"
      className={`nav-item ${page === current ? "active" : ""}`}
      onClick={() => onClick(page)}
    >
      {icon}
      {label}
    </button>
  );
}

const STEPS = [
  {
    title: "推到边缘就切",
    body: "把鼠标推到屏幕最外侧边缘的角落区，停大约 0.1 秒，就会切到上一个 / 下一个虚拟桌面。",
  },
  {
    title: "不会误触",
    body: "内部接缝、任务栏、全屏应用、拖拽中的鼠标都会被跳过；停住不动也不会连续切换。",
  },
  {
    title: "先试一下",
    body: "点“开始”会打开测试模式：布局图会高亮命中位置，但不会真的切换。确认位置对了再关掉。",
  },
];

function Onboarding({
  step,
  onNext,
  onFinish,
  onSkip,
}: {
  step: number;
  onNext: () => void;
  onFinish: () => void;
  onSkip: () => void;
}) {
  const current = STEPS[Math.min(step, STEPS.length - 1)];
  const last = step >= STEPS.length - 1;

  return (
    <div className="modal-backdrop" role="dialog" aria-modal="true" aria-label="首次使用引导">
      <div className="modal">
        <div className="big-icon">
          <Sparkles size={20} />
        </div>
        <h2>{current.title}</h2>
        <p>{current.body}</p>

        <div className="steps">
          {STEPS.map((_, index) => (
            <div key={index} className={`step-dot ${index <= step ? "on" : ""}`} />
          ))}
        </div>

        <div className="modal-actions">
          <button type="button" className="btn" onClick={onSkip}>
            跳过
          </button>
          {last ? (
            <button type="button" className="btn primary" onClick={onFinish}>
              开始
            </button>
          ) : (
            <button type="button" className="btn primary" onClick={onNext}>
              下一步
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
