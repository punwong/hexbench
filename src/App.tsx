import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import type { ReactNode } from "react";
import {
  Crosshair,
  Gauge,
  Info,
  Keyboard,
  LayoutGrid,
  MousePointer2,
  Rocket,
  Settings,
  Sparkles,
  AlertTriangle,
  ArrowUpCircle,
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
  showTriggerOverlay,
  takePendingPage,
} from "./api";
import type { Config, Snapshot } from "./types";
import {
  AboutPage,
  EdgePage,
  LauncherPage,
  DesktopPage,
  GeneralPage,
  GesturePage,
  HotkeysPage,
  Overview,
} from "./pages";
import type { Patch } from "./pages";
import { TitleBar } from "./components/TitleBar";

type Page =
  | "overview"
  | "edge"
  | "launcher"
  | "desktop"
  | "gestures"
  | "hotkeys"
  | "general"
  | "about";

/// Where the updater currently is: nothing, a version to grab, up to date,
/// downloading, a package waiting for a restart, or a failure worth showing.
type UpdatePhase =
  | "idle"
  | "checking"
  | "available"
  | "current"
  | "downloading"
  | "ready"
  | "error";

const TITLES: Record<Page, { title: string; subtitle: string }> = {
  overview: { title: "概览", subtitle: "" },
  edge: { title: "边缘桌面", subtitle: "触发位置、时序与防误触" },
  launcher: { title: "快捷面板", subtitle: "找文件、搬文件、写便签" },
  desktop: { title: "桌面格子", subtitle: "把文件夹钉在桌面上" },
  gestures: {
    title: "手势",
    subtitle: "在文件区按住右键滑动",
  },
  hotkeys: { title: "热键", subtitle: "" },
  general: { title: "通用", subtitle: "启动、外观与配置" },
  about: { title: "关于", subtitle: "更新、链接与反馈" },
};

export default function App() {
  const [page, setPage] = useState<Page>("overview");
  const mainRef = useRef<HTMLElement>(null);
  const [launcherTab, setLauncherTab] = useState<
    "items" | "folders" | "sites" | "notes"
  >("items");
  const applyNavigate = useCallback((payload: { page: string; tab: string | null } | null) => {
    const target = payload?.page ?? "overview";
    const tab = payload?.tab ?? null;
    if (
      target === "launcher" &&
      (tab === "items" || tab === "folders" || tab === "sites" || tab === "notes")
    ) {
      setLauncherTab(tab as "items" | "folders" | "sites" | "notes");
    }
    setPage(target as Page);
  }, []);

  // 换页回到顶部：滚动容器是 .main，不重置的话新页面会停在上
  // 一页的滚动位置，从半截卡片开始看。
  useEffect(() => {
    const main = mainRef.current;
    if (main) main.scrollTop = 0;
  }, [page]);

  // 面板里点「去设置」：切到指定页面（可带 tab）。
  useEffect(() => {
    let alive = true;
    let dispose: (() => void) | undefined;
    void listen<{ page: string; tab: string | null } | null>("navigate", (event) => {
      applyNavigate(event.payload);
    }).then((fn) => {
      // 监听是异步拿到的：effect 可能已经拆掉（开发模式下还会连跑两遍），
      // 这时候再挂上就永远没人注销了。
      if (alive) dispose = fn;
      else fn();
    });
    return () => {
      alive = false;
      dispose?.();
    };
  }, [applyNavigate]);
  // 这次窗口是「点设置」临时建出来的：真正要打开的那一页存在后端，
  // 挂载时来取一次——之前主窗口不在时那个事件发给空气，点了没反应。
  useEffect(() => {
    void takePendingPage()
      .then((pending) => {
        if (pending) applyNavigate({ page: pending[0], tab: pending[1] });
      })
      .catch(() => {});
  }, [applyNavigate]);
  const [snap, setSnap] = useState<Snapshot | null>(null);
  const [config, setConfig] = useState<Config | null>(null);
  const [error, setError] = useState<string | null>(null);
  // 设置是防抖静默保存的：给用户一个「正在保存 / 已保存」的回音，
  // 免得改完开关不知道到底写没写进去。
  const [saveState, setSaveState] = useState<"idle" | "saving" | "saved">("idle");
  const saveStateTimer = useRef<number | undefined>(undefined);

  const [updatePhase, setUpdatePhase] = useState<UpdatePhase>("idle");
  const [updateVersion, setUpdateVersion] = useState<string | null>(null);
  const [updateProgress, setUpdateProgress] = useState<number | null>(null);
  const [updateError, setUpdateError] = useState<string | null>(null);
  const [updateNotes, setUpdateNotes] = useState("");
  const [updateDate, setUpdateDate] = useState("");
  const [updateCheckedAt, setUpdateCheckedAt] = useState<number | null>(null);

  // 再查一次的时候要不要保留当前进度：包已经下好（ready）或正在下的时候
  // 一律不回到 checking —— 否则查完又变成 available，会重新下载一遍，
  // 「更新并重启」也跟着消失。
  const updatePhaseRef = useRef<UpdatePhase>("idle");
  useEffect(() => {
    updatePhaseRef.current = updatePhase;
  }, [updatePhase]);

  const runCheck = useCallback(async (silent: boolean) => {
    const stable =
      updatePhaseRef.current === "downloading" || updatePhaseRef.current === "ready";
    if (!stable) setUpdatePhase("checking");
    setUpdateError(null);
    try {
      const info = await checkUpdate();
      if (!info) {
        if (!stable) {
          setUpdateVersion(null);
          setUpdateNotes("");
          setUpdateDate("");
          setUpdatePhase("current");
        }
        setUpdateCheckedAt(Date.now());
        return;
      }
      setUpdateVersion(info.version);
      setUpdateNotes(info.notes ?? "");
      setUpdateDate(info.date ?? "");
      setUpdateCheckedAt(Date.now());
      if (!stable) setUpdatePhase("available");
    } catch (reason) {
      setUpdateError(String(reason));
      // A failed automatic check should stay quiet; a manual one should not.
      if (!stable) setUpdatePhase(silent ? "idle" : "error");
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

    const applyStatus = (status: {
      version: string | null;
      error?: string | null;
      checkedAt?: number;
      notes?: string | null;
      date?: string | null;
    }) => {
      if (!alive) return;
      if (status.checkedAt && status.checkedAt > 0) {
        setUpdateCheckedAt(status.checkedAt * 1000);
      }
      if (status.version) {
        setUpdateVersion(status.version);
        setUpdateNotes(status.notes ?? "");
        setUpdateDate(status.date ?? "");
        setUpdateError(null);
        setUpdatePhase((phase) =>
          phase === "downloading" || phase === "ready" ? phase : "available",
        );
      } else {
        setUpdateError(status.error ?? null);
        setUpdatePhase((phase) => {
          if (phase === "downloading" || phase === "ready") return phase;
          // 查过（哪怕没查到）就算「已是最新」，没查过才什么都不说。
          return status.checkedAt && status.checkedAt > 0 ? "current" : "idle";
        });
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
  const saveInFlight = useRef(false);
  // 保存期间后端发来的 config-changed 不能直接丢掉：存完要把后端那份重读回来，
  // 否则下一次保存还会拿旧快照把同步设置之类的字段盖回去。
  const configDirty = useRef(false);

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
      // The layout diagram polls the cursor on its own; this only refreshes
      // config/stats, so it can be slow.
      if (alive) timer = window.setTimeout(poll, 500);
    };

    void poll();
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, []);



  // 后端有些命令会绕过前端直接改配置（同步设置、面板折叠）。
  // 收到通知就重读一份，否则这边手里那份旧配置在下次保存时会把刚改的
  // 字段整份盖回去——比如填好的 WebDAV 地址、刚编辑的话术。
  useEffect(() => {
    let alive = true;
    let dispose: (() => void) | undefined;
    void listen("config-changed", () => {
      // 自己刚改的还没落盘：等这笔存完再说，别把正在输入的盖掉。
      if (saveTimer.current !== undefined || saveInFlight.current) {
        configDirty.current = true;
        return;
      }
      void getSnapshot()
        .then((next) => {
          if (!alive) return;
          setSnap(next);
          configRef.current = next.config;
          setConfig(next.config);
        })
        .catch(() => {});
    }).then((fn) => {
      if (alive) dispose = fn;
      else fn();
    });
    return () => {
      alive = false;
      dispose?.();
    };
  }, []);

  const update = useCallback((input: Patch | ((current: Config) => Patch | null)) => {
    const current = configRef.current;
    if (!current) return;
    // 也接受「按最新配置算补丁」的写法：网络请求之类的异步回调里，
    // 闭包拿到的列表可能已经过时，整份写回去会把中间做过的改动冲掉。
    const patch = typeof input === "function" ? input(current) : input;
    if (!patch) return;
    const next: Config = {
      ...current,
      ...patch,
      trigger: { ...current.trigger, ...patch.trigger },
      guard: { ...current.guard, ...patch.guard },
      hotkeys: { ...current.hotkeys, ...patch.hotkeys },
      launcher: { ...current.launcher, ...patch.launcher },
      notes: { ...current.notes, ...patch.notes },
      // 桌面格子整段也要展开合并：漏了这一行，每次保存都会把 desktop 丢掉。
      desktop: { ...current.desktop, ...patch.desktop },
      updates: { ...current.updates, ...patch.updates },
      ui: { ...current.ui, ...patch.ui },
    };
    configRef.current = next;
    setConfig(next);
    setSaveState("saving");
    window.clearTimeout(saveStateTimer.current);
    window.clearTimeout(saveTimer.current);
    saveTimer.current = window.setTimeout(() => {
      saveTimer.current = undefined;
      saveInFlight.current = true;
      const settle = () => {
        saveInFlight.current = false;
        // 保存期间又确认了一次更新：等下一笔保存结束时再重读，
        // 免得把用户正在输的内容换掉。
        if (!configDirty.current || saveTimer.current !== undefined) return;
        configDirty.current = false;
        void getSnapshot()
          .then((fresh) => {
            configRef.current = fresh.config;
            setConfig(fresh.config);
            setSnap(fresh);
          })
          .catch(() => {});
      };
      void saveConfig(next)
        .then(() => {
          setError(null);
          setSaveState("saved");
          saveStateTimer.current = window.setTimeout(() => setSaveState("idle"), 1600);
        })
        .catch((reason) => {
          setError(String(reason));
          setSaveState("idle");
        })
        .then(settle);
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

  // 引导里选哪个工具就进哪一页，进去就能直接配置。
  const finishOnboarding = (target: "launcher" | "edge" | "desktop") => {
    update({ ui: { onboarded: true } });
    setPage(target);
  };

  const skipOnboarding = () => {
    update({ ui: { onboarded: true } });
  };

  const status = snap
    ? snap.paused
      ? "已暂停"
      : config?.enabled
        ? "运行中"
        : "已关闭"
    : "加载中";
  const statusClass = snap?.paused ? "warn" : config?.enabled ? "ok" : "";
  // 概览列出三个工具，状态就别只报边缘那一个。
  const enabledTools = config
    ? [config.enabled, config.launcher.enabled, config.desktop.enabled].filter(Boolean).length
    : 0;

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

        <div className="nav-group">
          <div className="nav-label">工具</div>
          {/* 工具关掉了也留着入口：设置里才能把它再打开。 */}
          <NavItem page="launcher" current={page} onClick={setPage} icon={<Rocket size={16} />} label="快捷面板" />
          <NavItem page="edge" current={page} onClick={setPage} icon={<Crosshair size={16} />} label="边缘桌面" />
          <NavItem page="desktop" current={page} onClick={setPage} icon={<LayoutGrid size={16} />} label="桌面格子" />
        </div>

        <div className="nav-group">
          <div className="nav-label">应用</div>
          <NavItem page="hotkeys" current={page} onClick={setPage} icon={<Keyboard size={16} />} label="热键" />
          {/* 快捷面板和桌面格子共用一份手势设置，所以不算某一个工具的，跟热键一起放在这里。 */}
          <NavItem page="gestures" current={page} onClick={setPage} icon={<MousePointer2 size={16} />} label="手势" />
          <NavItem page="general" current={page} onClick={setPage} icon={<Settings size={16} />} label="通用" />
          <NavItem page="about" current={page} onClick={setPage} icon={<Info size={16} />} label="关于" />
        </div>

        <div className="sidebar-bottom">
        {showUpdatePill && (
          <div className={`update-pill ${updatePhase}`}>
            {updatePhase === "ready" ? (
              <>
                <span
                  className="update-version"
                  title={`新版本 ${updateVersion} 已下载，更新会重启应用`}
                >
                  新版本 {updateVersion}
                </span>
                <button
                  type="button"
                  className="btn primary update-install"
                  title="安装新版本并重启"
                  onClick={() => void runInstall()}
                >
                  <ArrowUpCircle size={13} strokeWidth={2.2} />
                  更新
                </button>
                <button
                  type="button"
                  className="update-dismiss"
                  title="忽略这个版本"
                  aria-label="忽略这个版本"
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

      <main className="main" ref={mainRef}>
        <header className="page-head">
          <div>
            <h1>{TITLES[page].title}</h1>
            {TITLES[page].subtitle && (
              <div className="sub">{TITLES[page].subtitle}</div>
            )}
          </div>
          <div className="page-head-side">
            {saveState !== "idle" && (
              <span className={`save-state ${saveState}`} aria-live="polite">
                {saveState === "saving" ? "保存中…" : "已保存"}
              </span>
            )}
            {page === "overview" ? (
              <div className="status" title="三个工具当前的启用情况">
                <i className={`dot ${enabledTools === 3 ? "ok" : ""}`} />
                {enabledTools} / 3 个工具已启用
              </div>
            ) : page === "edge" ? (
              <div className="status" title="边缘切换引擎的状态">
                <i className={`dot ${statusClass}`} />
                边缘切换 · {status}
              </div>
            ) : null}
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

        ) : page === "desktop" ? (
          <DesktopPage config={config} update={update} monitors={snap.monitors} />
        ) : page === "gestures" ? (
          <GesturePage config={config} update={update} />
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
        ) : (
          <AboutPage
            snap={snap}
            updateState={{
              phase: updatePhase,
              version: updateVersion,
              progress: updateProgress,
              error: updateError,
              notes: updateNotes,
              date: updateDate,
              checkedAt: updateCheckedAt,
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
          hotkey={config?.hotkeys.launcher ?? ""}
          onPick={finishOnboarding}
          onSkip={skipOnboarding}
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

/** 首次引导：三个工具各给一条入口，而不是只讲边缘切换。 */
const ONBOARD_TOOLS: Array<{
  target: "launcher" | "edge" | "desktop";
  icon: ReactNode;
  name: string;
  hint: string;
  action: string;
}> = [
  {
    target: "launcher",
    icon: <Rocket size={18} />,
    name: "快捷面板",
    hint: "一键呼出：找文件、搬文件、写便签",
    action: "设置",
  },
  {
    target: "edge",
    icon: <Crosshair size={18} />,
    name: "边缘桌面",
    hint: "鼠标贴到屏幕最外侧边缘，切换虚拟桌面",
    action: "设置",
  },
  {
    target: "desktop",
    icon: <LayoutGrid size={18} />,
    name: "桌面格子",
    hint: "把常用文件夹钉在桌面上，拖进去就算整理好",
    action: "设置",
  },
];

function Onboarding({
  hotkey,
  onPick,
  onSkip,
}: {
  hotkey: string;
  onPick: (target: "launcher" | "edge" | "desktop") => void;
  onSkip: () => void;
}) {
  return (
    <div className="modal-backdrop" role="dialog" aria-modal="true" aria-label="首次使用引导">
      <div className="modal onboarding">
        <div className="big-icon">
          <Sparkles size={20} />
        </div>
        <h2>三个工具，各管一段</h2>
        <p>三个可以同时用，之后随时能在概览里开关。</p>

        <div className="onboard-list">
          {ONBOARD_TOOLS.map((tool) => (
            <button
              type="button"
              key={tool.target}
              className="onboard-row"
              onClick={() => onPick(tool.target)}
            >
              <span className="onboard-icon">{tool.icon}</span>
              <span className="onboard-text">
                <span className="onboard-name">
                  {tool.name}
                  {tool.target === "launcher" && hotkey && <kbd>{hotkey}</kbd>}
                </span>
                <span className="onboard-hint">{tool.hint}</span>
              </span>
              <span className="onboard-go">{tool.action}</span>
            </button>
          ))}
        </div>

        <div className="modal-actions">
          <button type="button" className="btn" onClick={onSkip}>
            跳过
          </button>
        </div>
      </div>
    </div>
  );
}
