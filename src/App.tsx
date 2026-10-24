import { useCallback, useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import {
  Crosshair,
  Gauge,
  Info,
  Keyboard,
  MessageSquare,
  Settings,
  ShieldCheck,
  Sparkles,
} from "lucide-react";

import {
  createVirtualDesktop,
  getSnapshot,
  openConfigFolder,
  refreshLayout,
  saveConfig,
  setAutostart,
  setPaused,
  setTestMode,
  showTriggerOverlay,
} from "./api";
import type { Config, Snapshot } from "./types";
import {
  AboutPage,
  GeneralPage,
  GuardsPage,
  HotkeysPage,
  Overview,
  PhrasesPage,
  TriggerPage,
} from "./pages";
import type { Patch } from "./pages";

type Page = "overview" | "trigger" | "guards" | "phrases" | "hotkeys" | "general" | "about";

const TITLES: Record<Page, { title: string; subtitle: string }> = {
  overview: { title: "概览", subtitle: "实时状态与显示器布局" },
  trigger: { title: "触发", subtitle: "触发带位置、比例与时序" },
  guards: { title: "防误触", subtitle: "哪些情况下不触发" },
  phrases: { title: "快捷语", subtitle: "吸附在聊天窗口旁，点一下就发出去" },
  hotkeys: { title: "热键", subtitle: "所有全局快捷键，集中在一处" },
  general: { title: "通用", subtitle: "外观、启动与配置文件" },
  about: { title: "关于", subtitle: "版本、许可证与诊断" },
};

export default function App() {
  const [page, setPage] = useState<Page>("overview");
  const [snap, setSnap] = useState<Snapshot | null>(null);
  const [config, setConfig] = useState<Config | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [step, setStep] = useState(0);

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
      if (alive) timer = window.setTimeout(poll, 120);
    };

    void poll();
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, []);

  // Follow the Windows accent color when the user picked a chromatic one.
  // `accent` is null for gray/black accents, and then the built-in blue stays.
  useEffect(() => {
    const accent = snap?.accent;
    if (accent) {
      document.documentElement.style.setProperty("--accent", accent);
    } else {
      document.documentElement.style.removeProperty("--accent");
    }
  }, [snap?.accent]);


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

  return (
    <div className="app">
      <aside className="sidebar">
        <div className="brand">
          <div className="logo" />
          <div>
            <div className="brand-name">HexBench</div>
            <div className="brand-sub">海克斯</div>
          </div>
        </div>

        <div className="nav-group">
          <div className="nav-label">边缘切桌面</div>
          <NavItem page="overview" current={page} onClick={setPage} icon={<Gauge size={16} />} label="概览" />
          <NavItem page="trigger" current={page} onClick={setPage} icon={<Crosshair size={16} />} label="触发" />
          <NavItem page="guards" current={page} onClick={setPage} icon={<ShieldCheck size={16} />} label="防误触" />
        </div>

        <div className="nav-group">
          <div className="nav-label">快捷语</div>
          <NavItem page="phrases" current={page} onClick={setPage} icon={<MessageSquare size={16} />} label="话术面板" />
        </div>

        <div className="nav-group">
          <div className="nav-label">全局</div>
          <NavItem page="hotkeys" current={page} onClick={setPage} icon={<Keyboard size={16} />} label="热键" />
          <NavItem page="general" current={page} onClick={setPage} icon={<Settings size={16} />} label="通用" />
          <NavItem page="about" current={page} onClick={setPage} icon={<Info size={16} />} label="关于" />
        </div>

        <div className="sidebar-foot">v{snap?.version ?? "0.1.0"} · MIT</div>
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

        {error && <div className="error">保存失败：{error}</div>}

        {!snap || !config ? (
          <div className="card">正在读取状态…</div>
        ) : page === "overview" ? (
          <Overview
            snap={snap}
            config={config}
            update={update}
            onPause={() => void setPaused(!snap.paused)}
            onTest={(value) => void setTestMode(value)}
            onCreateDesktop={() => {
              void createVirtualDesktop().catch((reason) => setError(String(reason)));
            }}
          />
        ) : page === "trigger" ? (
          <TriggerPage
            config={config}
            update={update}
            monitors={snap.monitors}
            onPreview={() => {
              void showTriggerOverlay().catch((reason) => setError(String(reason)));
            }}
          />
        ) : page === "guards" ? (
          <GuardsPage config={config} update={update} />
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
        ) : (
          <AboutPage snap={snap} />
        )}
      </main>

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
