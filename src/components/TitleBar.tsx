import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";

type ResizeDirection =
  | "East"
  | "North"
  | "NorthEast"
  | "NorthWest"
  | "South"
  | "SouthEast"
  | "SouthWest"
  | "West";

/** Frameless window: the whole app draws its own title bar, so the frame has to
 *  provide the edges Windows would otherwise give us. */
const EDGES: Array<[string, ResizeDirection]> = [
  ["n", "North"],
  ["s", "South"],
  ["e", "East"],
  ["w", "West"],
  ["ne", "NorthEast"],
  ["nw", "NorthWest"],
  ["se", "SouthEast"],
  ["sw", "SouthWest"],
];

export function TitleBar() {
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    const win = getCurrentWindow();
    let dispose: (() => void) | undefined;
    let alive = true;
    const sync = () => {
      void win.isMaximized().then((value) => {
        if (alive) setMaximized(value);
      });
    };
    sync();
    void win
      .onResized(sync)
      .then((fn) => {
        dispose = fn;
      });
    return () => {
      alive = false;
      dispose?.();
    };
  }, []);

  const win = getCurrentWindow();

  return (
    <>
      {EDGES.map(([edge, direction]) => (
        <div
          key={edge}
          className={`rz rz-${edge}`}
          onPointerDown={() => void win.startResizeDragging(direction)}
        />
      ))}

      <header className="titlebar" data-tauri-drag-region>
        <span className="titlebar-brand" data-tauri-drag-region>
          <span className="titlebar-logo" aria-hidden="true" />
          <span className="titlebar-name" data-tauri-drag-region>
            HexBench
          </span>
          <span className="titlebar-sub" data-tauri-drag-region>
            海克斯
          </span>
        </span>
        <div className="titlebar-actions">
          <button
            type="button"
            className="tb-btn"
            aria-label="最小化"
            onClick={() => void win.minimize()}
          >
            <svg viewBox="0 0 10 10" width="10" height="10" aria-hidden="true">
              <path d="M2 5h6" stroke="currentColor" strokeWidth="1" />
            </svg>
          </button>
          <button
            type="button"
            className="tb-btn"
            aria-label={maximized ? "还原" : "最大化"}
            onClick={() => void win.toggleMaximize()}
          >
            {maximized ? (
              <svg viewBox="0 0 10 10" width="10" height="10" aria-hidden="true">
                <path d="M2.5 4h3.5v3.5H2.5z" fill="none" stroke="currentColor" strokeWidth="1" />
                <path d="M4 4V2.5h3.5V6" fill="none" stroke="currentColor" strokeWidth="1" />
              </svg>
            ) : (
              <svg viewBox="0 0 10 10" width="10" height="10" aria-hidden="true">
                <rect
                  x="2.5"
                  y="2.5"
                  width="5"
                  height="5"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1"
                />
              </svg>
            )}
          </button>
          <button
            type="button"
            className="tb-btn close"
            aria-label="关闭"
            onClick={() => void win.close()}
          >
            <svg viewBox="0 0 10 10" width="10" height="10" aria-hidden="true">
              <path d="M2.5 2.5l5 5M7.5 2.5l-5 5" stroke="currentColor" strokeWidth="1" />
            </svg>
          </button>
        </div>
      </header>
    </>
  );
}
