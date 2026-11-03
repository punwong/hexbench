import {
  useCallback,
  useEffect,
  useRef,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from "react";

/// 文件和格子的鼠标手势：按住右键左右滑动 = 后退 / 前进。
/// 快捷面板和桌面格子共用同一套设置（设置页的「手势」），只差别在历史栈由谁维护。
export type GestureSettings = {
  enabled: boolean;
  invertDirection: boolean;
  triggerDistance: number;
  showTrail: boolean;
};

/// 手势进行中的状态。全放在 ref 里：pointermove 每帧都要读，走 React 状态会重渲染整张列表。
type GestureRuntime = {
  active: boolean;
  pointerId: number;
  startX: number;
  startY: number;
  lastX: number;
  lastY: number;
  /** 已经判定为横向手势（超过起步距离且横向占优） */
  recognized: boolean;
  direction: "back" | "forward" | null;
  points: Array<{ x: number; y: number }>;
  /** contextmenu 抑制截止时间（performance.now()） */
  suppressUntil: number;
  line: string;
  glow: string;
  raf: number;
  hideTimer: number;
};

const GESTURE_START = 8;
/// 手势结束后还要按住右键菜单一小会儿：WebView2 的 contextmenu 在松开之后才来。
const GESTURE_SUPPRESS_MS = 600;

export const GESTURE_DEFAULT: GestureSettings = {
  enabled: true,
  invertDirection: false,
  triggerDistance: 80,
  showTrail: true,
};

/** 横向位移决定动作；默认向左后退，开了「左右互换」就反过来。 */
function gestureAction(dx: number, invert: boolean): "back" | "forward" {
  if (dx < 0) return invert ? "forward" : "back";
  return invert ? "back" : "forward";
}

/** 配置文件被手改坏也不至于滑半天不动：夹到 20–400px。 */
function gestureDistance(value: number): number {
  return Number.isFinite(value) ? Math.min(400, Math.max(20, value)) : 80;
}

export type GestureOptions = {
  settings: GestureSettings;
  /** 历史里还能退 / 还能进的层数：没有可去的地方时方向提示画淡一点。 */
  backCount: number;
  forwardCount: number;
  onBack: () => void;
  onForward: () => void;
  /** 已经到头了：提示文字由页面决定。 */
  onEmpty: (direction: "back" | "forward") => void;
  /** 光标旁的方向文字；不给就是面板那套「后退 / 前进」。 */
  labels?: { back: string; forward: string };
};

/**
 * 右键手势：把 `begin` 挂在文件区的 pointerdown 上，把返回的 `layer` 放进页面，
 * 剩下的（窗口级移动 / 松开 / Esc / 菜单抑制）都由这个钩子自己管。
 */
export function useMouseGestures(options: GestureOptions) {
  // 窗口级监听只装一次，设置、历史层数和回调都从 ref 里读最新值。
  const latest = useRef(options);
  latest.current = options;

  const gestureRef = useRef<GestureRuntime>({
    active: false,
    pointerId: -1,
    startX: 0,
    startY: 0,
    lastX: 0,
    lastY: 0,
    recognized: false,
    direction: null,
    points: [],
    suppressUntil: 0,
    line: "rgba(0, 0, 0, 0.45)",
    glow: "rgba(0, 0, 0, 0.10)",
    raf: 0,
    hideTimer: 0,
  });
  const layerRef = useRef<HTMLDivElement | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const labelRef = useRef<HTMLDivElement | null>(null);

  /** 按需重设 canvas 的背板尺寸（换屏幕 / 缩放变了也要跟上）。 */
  const gestureCtx = () => {
    const canvas = canvasRef.current;
    if (!canvas) return null;
    const dpr = window.devicePixelRatio || 1;
    const width = Math.round(window.innerWidth * dpr);
    const height = Math.round(window.innerHeight * dpr);
    if (canvas.width !== width || canvas.height !== height) {
      canvas.width = width;
      canvas.height = height;
    }
    const ctx = canvas.getContext("2d");
    if (!ctx) return null;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    return ctx;
  };

  const drawGesture = useCallback(() => {
    const ctx = gestureCtx();
    if (!ctx) return;
    ctx.clearRect(0, 0, window.innerWidth, window.innerHeight);
    const points = gestureRef.current.points;
    if (points.length === 0) return;
    const stroke = (width: number, color: string) => {
      ctx.lineWidth = width;
      ctx.strokeStyle = color;
      ctx.lineCap = "round";
      ctx.lineJoin = "round";
      ctx.beginPath();
      if (points.length === 1) {
        ctx.moveTo(points[0].x, points[0].y);
        ctx.lineTo(points[0].x + 0.01, points[0].y);
      } else {
        ctx.moveTo(points[0].x, points[0].y);
        // 经过每个点的中点画二次曲线：采样点再密，线也不会一段一段的。
        for (let index = 1; index < points.length - 1; index += 1) {
          const midX = (points[index].x + points[index + 1].x) / 2;
          const midY = (points[index].y + points[index + 1].y) / 2;
          ctx.quadraticCurveTo(points[index].x, points[index].y, midX, midY);
        }
        const last = points[points.length - 1];
        ctx.lineTo(last.x, last.y);
      }
      ctx.stroke();
    };
    stroke(9, gestureRef.current.glow);
    stroke(3, gestureRef.current.line);
  }, []);

  const scheduleGestureDraw = useCallback(() => {
    const g = gestureRef.current;
    if (g.raf) return;
    g.raf = window.requestAnimationFrame(() => {
      g.raf = 0;
      drawGesture();
    });
  }, [drawGesture]);

  const updateGestureLabel = useCallback((x: number, y: number) => {
    const label = labelRef.current;
    const g = gestureRef.current;
    if (!label || !g.direction) return;
    const back = g.direction === "back";
    const state = latest.current;
    const available = back ? state.backCount > 0 : state.forwardCount > 0;
    const text = back
      ? (state.labels?.back ?? "← 后退")
      : (state.labels?.forward ?? "→ 前进");
    if (label.textContent !== text) label.textContent = text;
    label.classList.toggle("dim", !available);
    label.hidden = false;
    const left = Math.min(Math.max(x + 18, 8), window.innerWidth - label.offsetWidth - 8);
    const top = Math.min(Math.max(y + 16, 8), window.innerHeight - label.offsetHeight - 8);
    label.style.transform = `translate3d(${left}px, ${top}px, 0)`;
  }, []);

  const showGestureLayer = useCallback(
    (x: number, y: number) => {
      const layer = layerRef.current;
      if (!layer) return;
      const g = gestureRef.current;
      const style = getComputedStyle(document.documentElement);
      g.line = style.getPropertyValue("--gesture-line").trim() || "rgba(0, 0, 0, 0.45)";
      g.glow = style.getPropertyValue("--gesture-glow").trim() || "rgba(0, 0, 0, 0.10)";
      g.points = [{ x, y }];
      window.clearTimeout(g.hideTimer);
      layer.hidden = false;
      // 先落地一帧再挂 .on，透明度才会从 0 过渡出来。
      void layer.offsetWidth;
      layer.classList.add("on");
      updateGestureLabel(x, y);
    },
    [updateGestureLabel],
  );

  const hideGestureLayer = useCallback(() => {
    const layer = layerRef.current;
    const g = gestureRef.current;
    if (!layer || layer.hidden) return;
    layer.classList.remove("on");
    const clear = () => {
      const ctx = gestureCtx();
      ctx?.clearRect(0, 0, window.innerWidth, window.innerHeight);
      layer.hidden = true;
    };
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      clear();
      return;
    }
    window.clearTimeout(g.hideTimer);
    g.hideTimer = window.setTimeout(clear, 180);
  }, []);

  /** 结束（或取消）当前手势。commit=false 用于 Esc / 失焦 / 呼出重置。 */
  const finishGesture = useCallback(
    (commit: boolean) => {
      const g = gestureRef.current;
      if (!g.active) return;
      const recognized = g.recognized;
      const direction = g.direction;
      const dx = g.lastX - g.startX;
      g.active = false;
      g.pointerId = -1;
      if (g.raf) {
        window.cancelAnimationFrame(g.raf);
        g.raf = 0;
      }
      if (!recognized) return;
      // WebView2 的 contextmenu 在右键松开之后才发：抑制要多留一会儿。
      g.suppressUntil = performance.now() + GESTURE_SUPPRESS_MS;
      hideGestureLayer();
      g.points = [];
      if (!commit || !direction) return;
      const state = latest.current;
      if (Math.abs(dx) < gestureDistance(state.settings.triggerDistance)) return;
      const available = direction === "back" ? state.backCount : state.forwardCount;
      if (available <= 0) {
        state.onEmpty(direction);
        return;
      }
      if (direction === "back") state.onBack();
      else state.onForward();
    },
    [hideGestureLayer],
  );

  const cancelGesture = useCallback(() => {
    const g = gestureRef.current;
    if (g.raf) {
      window.cancelAnimationFrame(g.raf);
      g.raf = 0;
    }
    g.active = false;
    g.pointerId = -1;
    g.recognized = false;
    g.direction = null;
    g.points = [];
    g.suppressUntil = 0;
    hideGestureLayer();
  }, [hideGestureLayer]);

  /** 文件区按下右键：只记起点，不动菜单——不划动时右键菜单照常弹。 */
  const begin = (event: ReactPointerEvent) => {
    if (event.button !== 2 || event.pointerType !== "mouse") return;
    if (!latest.current.settings.enabled) return;
    const g = gestureRef.current;
    if (g.active) return;
    g.suppressUntil = 0;
    g.active = true;
    g.pointerId = event.pointerId;
    g.startX = event.clientX;
    g.startY = event.clientY;
    g.lastX = event.clientX;
    g.lastY = event.clientY;
    g.recognized = false;
    g.direction = null;
    g.points = [];
  };

  useEffect(() => {
    const g = gestureRef.current;
    const down = () => {
      // 新的按下：上一次手势留下的菜单抑制立刻作废，普通右键不能被永久吞掉。
      if (!g.active) g.suppressUntil = 0;
    };
    const move = (event: PointerEvent) => {
      if (!g.active || event.pointerId !== g.pointerId) return;
      if ((event.buttons & 2) === 0) {
        // 松开事件丢了（拖到窗口外松手）：按已经划到的位置收尾。
        finishGesture(true);
        return;
      }
      const settings = latest.current.settings;
      g.lastX = event.clientX;
      g.lastY = event.clientY;
      const dx = g.lastX - g.startX;
      const dy = g.lastY - g.startY;
      if (!g.recognized) {
        if (Math.hypot(dx, dy) < GESTURE_START) return;
        // 竖着划不算：右键上下拖更像是在选一行，不把人导航走。
        if (Math.abs(dx) < Math.abs(dy) * 1.2) return;
        g.recognized = true;
        g.suppressUntil = performance.now() + 1500;
        if (settings.showTrail) showGestureLayer(g.startX, g.startY);
      }
      g.direction = gestureAction(dx, settings.invertDirection);
      if (settings.showTrail) {
        const last = g.points[g.points.length - 1];
        if (!last || Math.hypot(g.lastX - last.x, g.lastY - last.y) >= 2.5) {
          g.points.push({ x: g.lastX, y: g.lastY });
          // 划得再长也只留一段尾巴，别一直涨内存。
          if (g.points.length > 600) g.points.splice(1, 1);
        }
        updateGestureLabel(g.lastX, g.lastY);
        scheduleGestureDraw();
      }
    };
    const up = (event: PointerEvent) => {
      if (!g.active || event.pointerId !== g.pointerId) return;
      g.lastX = event.clientX;
      g.lastY = event.clientY;
      finishGesture(true);
    };
    const cancel = () => finishGesture(false);
    const key = (event: KeyboardEvent) => {
      if (!g.active || event.key !== "Escape") return;
      // 手势里的 Esc 只是取消手势，不能顺带退一层 / 收起面板。
      event.preventDefault();
      event.stopPropagation();
      event.stopImmediatePropagation();
      finishGesture(false);
    };
    const menu = (event: MouseEvent) => {
      // 空白处的菜单监听在 document 上，且不查 defaultPrevented：
      // 必须在 window 的捕获阶段就把事件拦死，tile 上的 React 菜单也到不了。
      if ((g.active && g.recognized) || performance.now() < g.suppressUntil) {
        event.preventDefault();
        event.stopPropagation();
        event.stopImmediatePropagation();
      }
    };
    window.addEventListener("pointerdown", down, true);
    window.addEventListener("pointermove", move, true);
    window.addEventListener("pointerup", up, true);
    window.addEventListener("pointercancel", cancel, true);
    window.addEventListener("keydown", key, true);
    window.addEventListener("contextmenu", menu, true);
    window.addEventListener("blur", cancel);
    document.addEventListener("visibilitychange", cancel);
    return () => {
      window.removeEventListener("pointerdown", down, true);
      window.removeEventListener("pointermove", move, true);
      window.removeEventListener("pointerup", up, true);
      window.removeEventListener("pointercancel", cancel, true);
      window.removeEventListener("keydown", key, true);
      window.removeEventListener("contextmenu", menu, true);
      window.removeEventListener("blur", cancel);
      document.removeEventListener("visibilitychange", cancel);
    };
  }, [finishGesture, scheduleGestureDraw, showGestureLayer, updateGestureLabel]);

  /** 轨迹层：fixed 铺满窗口，永远不吃鼠标事件。 */
  const layer: ReactNode = (
    <div className="gesture-layer" ref={layerRef} hidden aria-hidden="true">
      <canvas className="gesture-canvas" ref={canvasRef} />
      <div className="gesture-label" ref={labelRef} hidden />
    </div>
  );

  return { begin, reset: cancelGesture, layer };
}
