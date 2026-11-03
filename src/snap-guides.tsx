import { useEffect, useState } from "react";
import ReactDOM from "react-dom/client";
import { listen } from "@tauri-apps/api/event";

/// 后端 `snap-guides` 事件：坐标已经换算成这块画布里的 CSS 像素。
type Segment = { from: number; to: number };
type Vertical = Segment & { x: number };
type Horizontal = Segment & { y: number };

type Payload = {
  v: Vertical | null;
  h: Horizontal | null;
};

const empty: Payload = { v: null, h: null };

function span(segment: Segment) {
  return {
    start: Math.min(segment.from, segment.to),
    length: Math.abs(segment.to - segment.from),
  };
}

function Guides() {
  const [lines, setLines] = useState<Payload>(empty);

  // 这层窗口没有任务栏入口、没有可见内容，出问题只能从窗口标题上分辨：
  // HGUIDE = 页面起来了，HGUIDE=v/h = 收到了事件。调试和截图测试都靠它。
  useEffect(() => {
    document.title = "HGUIDE";
  }, []);
  useEffect(() => {
    document.title = lines.v ? "HGUIDE v" : lines.h ? "HGUIDE h" : "HGUIDE -";
  }, [lines]);

  useEffect(() => {
    let dispose: (() => void) | undefined;
    // 这个 label 的窗口只有一个，但仍然指上 target：不指的话 Tauri 的 listen
    // 默认收所有目标，别的窗口的事件也会进来。
    void listen<Payload>(
      "snap-guides",
      (event) => setLines(event.payload ?? empty),
      { target: "snap-guides" },
    ).then((unlisten) => {
      dispose = unlisten;
    });
    return () => dispose?.();
  }, []);

  const vertical = lines.v ? span(lines.v) : null;
  const horizontal = lines.h ? span(lines.h) : null;

  return (
    <>
      {lines.v && vertical && (
        <div
          className="line v"
          style={{ left: lines.v.x, top: vertical.start, height: vertical.length }}
        />
      )}
      {lines.h && horizontal && (
        <div
          className="line h"
          style={{ top: lines.h.y, left: horizontal.start, width: horizontal.length }}
        />
      )}
    </>
  );
}

ReactDOM.createRoot(document.getElementById("root")!).render(<Guides />);
